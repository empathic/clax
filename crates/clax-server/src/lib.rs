//! HTTP server for Clax: REST API, content serving, SSE, embedded UI.
//!
//! # Two hosts, one server
//!
//! The server runs as the local daemon (natively, on tokio and hyper) and
//! inside a Cloudflare Durable Object (wasm32-unknown-unknown, through
//! workers-rs). The code is shared; the target decides what is built, by
//! `cfg(target_arch = "wasm32")`, with no cargo feature to pick a host: the
//! wasm32 build is checked by `scripts/check-wasm.sh -p clax-server`. The
//! Durable Object side (the Worker crate) is not built yet: what this says
//! of it is the contract it builds on.
//!
//! ## What both hosts run
//!
//! - The router ([`build_router`], [`routes::router`]), every handler that
//!   answers one request, and the middleware around them (host rewrite,
//!   `/api` host check, agent authentication, CORS, compression).
//! - [`AppState`] and everything it holds in memory: the store (on the
//!   Worker, one connection in the object's storage, `Store::open_on`, so
//!   [`AppState::store_call`] runs its job inline), the event bus, the
//!   stream hub ([`stream::Hub`]), presence, working records, rooms,
//!   questions and feedback waiters, grant and agent bounds.
//! - The MCP tool set ([`clax_mcp::ClaxTools`]) and its REST client.
//!
//! ## What only the daemon runs
//!
//! - Its lifecycle (`daemon`): the discovery file and start lock, port
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
//!   override directory) and the `testing` daemon.
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
//! [`seats::LiveSeats::sweep`], the agents' drafts'
//! [`drafts::sweep_and_announce`], grant expiry) and sets the
//! next alarm for the earliest one due. The request timeout layer is the
//! daemon's; a Worker's request is bounded by the platform.
//!
//! ## Live updates: one fan-out, two outlets
//!
//! Every event goes through [`stream::Hub::dispatch`] on the publishing
//! thread, which queues a ready-made frame for each subscriber of its
//! topic ([`stream::Opened`]'s bounded queue, with `resync` when a
//! subscriber falls behind). What drains a subscriber's queue is the
//! outlet:
//!
//! - natively, [`stream::Conn`] turns it into the SSE body of
//!   `GET /api/stream`, which holds its connection open;
//! - on the Worker, the Durable Object owns a hibernatable WebSocket per
//!   client, outside axum: it opens the client's stream with the same
//!   [`stream::Hub::open_for`], and changes its subscriptions with
//!   [`seats::subscribe`], which holds the live seats an artifact's live
//!   topics take, as `POST /api/stream/<id>` does (subscriptions arrive as
//!   socket messages instead), and
//!   after each request or alarm it drains every socket's queue without
//!   waiting (`try_recv`), sending each frame as one text message. An SSE
//!   stream would keep the object awake for as long as it is open; a
//!   hibernated socket costs nothing. The stream ID and topics ride in the
//!   socket's attachment, so an object woken from hibernation reopens each
//!   socket's stream and sends `resync` for its topics, as a resume past
//!   the hub's rings does.
//!
//! Rooms ([`room::Rooms`]) and presence follow the same rule: the hubs are
//! shared, and only the socket that carries their frames differs.
//!
//! ## MCP on the Worker
//!
//! rmcp 3.5's streamable HTTP service spawns each session, and in its
//! stateless mode each request, on tokio (`tokio::spawn` in
//! `transport/streamable_http_server/tower.rs` and in `serve_directly`),
//! and its `Peer` cannot be built outside rmcp, so no rmcp transport runs
//! in a Worker. There `/mcp` is to be a thin JSON-RPC adapter over the
//! same tool set (not built yet): `POST` with a JSON response, no session
//! and no SSE; `initialize`, `ping`, `tools/list` (from the tool router)
//! and `tools/call`, which
//! calls the named tool's method directly (the tools take no request
//! context). The daemon keeps rmcp's stateful service and the stdio shim
//! unchanged.

pub mod agent;
pub mod approvals;
pub mod audit;
pub mod auth;
pub mod blocking;
pub mod boot;
#[cfg(not(target_arch = "wasm32"))]
pub mod daemon;
pub mod db_caller;
pub mod drafts;
pub mod error;
pub mod extension;
pub mod feedback;
pub mod grant_auth;
pub mod host;
pub mod http_cache;
pub mod identity;
pub mod inbox;
pub mod live;
pub mod live_layers;
pub mod presence;
pub mod push;
pub mod questions;
pub mod room;
pub mod routes;
pub mod sample;
pub mod seats;
pub mod shell_route;
pub mod state;
pub mod stream;
#[cfg(all(feature = "test-support", not(target_arch = "wasm32")))]
pub mod testing;
pub mod viewer;
pub mod working;
pub mod wrap_cache;

pub use state::AppState;

pub fn build_router(state: AppState) -> axum::Router {
    wrap(state.clone(), routes::router(state, None))
}

pub fn build_router_with_shutdown(
    state: AppState,
    shutdown: tokio::sync::watch::Sender<bool>,
) -> axum::Router {
    wrap(state.clone(), routes::router(state, Some(shutdown)))
}

// `Router::layer` runs after route matching, so the host rewrite has to wrap the
// whole inner router (mounted as the outer fallback) to influence routing. The
// `/api` host check runs after the rewrite (the later `layer` is outermost),
// and the header checks of agent credentials after both, before any route.
fn wrap(state: AppState, inner: axum::Router) -> axum::Router {
    axum::Router::new()
        .fallback_service(inner)
        .layer(axum::middleware::from_fn_with_state(
            state,
            agent::authenticate,
        ))
        .layer(axum::middleware::from_fn(auth::require_api_host))
        .layer(axum::middleware::from_fn(host::rewrite_artifact_host))
}

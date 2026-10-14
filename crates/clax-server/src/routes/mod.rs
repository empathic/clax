pub mod approvals;
pub mod artifacts;
pub mod assets;
pub mod connect;
pub mod content;
pub mod docs;
#[cfg(not(target_arch = "wasm32"))]
pub mod events;
pub mod extension;
pub mod feedback;
pub mod grants;
pub mod health;
pub mod inbox;
pub mod live;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp;
pub mod people;
pub mod perf;
pub mod questions;
#[cfg(not(target_arch = "wasm32"))]
pub mod room;
pub mod sample;
pub mod sessions;
pub mod shell;
pub mod sites;
pub mod snapshot;
pub mod stream;
pub mod threads;
pub mod token;
pub mod toolpath;
pub mod viewers;
pub mod watches;
pub mod working;

use crate::auth::RequireToken;
#[cfg(not(target_arch = "wasm32"))]
use crate::error::ApiError;
use crate::state::AppState;
#[cfg(not(target_arch = "wasm32"))]
use axum::error_handling::HandleErrorLayer;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    handler::Handler,
    http::StatusCode,
    routing::{delete, get, post},
};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use tower::ServiceBuilder;
#[cfg(not(target_arch = "wasm32"))]
use tower::timeout::TimeoutLayer;
use tower_http::cors::{Any, CorsLayer};

#[cfg(not(target_arch = "wasm32"))]
async fn timeout_error(err: tower::BoxError, limit: Duration) -> ApiError {
    if err.is::<tower::timeout::error::Elapsed>() {
        ApiError::new(
            StatusCode::REQUEST_TIMEOUT,
            "timeout",
            format!("request exceeded {limit:?}"),
        )
    } else {
        tracing::error!(error = %err, "request middleware failed");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn with_timeout(router: Router<AppState>, d: Duration) -> Router<AppState> {
    router.layer(
        ServiceBuilder::new()
            .layer(HandleErrorLayer::new(move |e| timeout_error(e, d)))
            .layer(TimeoutLayer::new(d)),
    )
}

/// No deadline layer on wasm32: tower's timeout sleeps on tokio's timer, and
/// a Worker's request is bounded by the platform.
#[cfg(target_arch = "wasm32")]
fn with_timeout(router: Router<AppState>, _d: Duration) -> Router<AppState> {
    router
}

pub fn router(state: AppState, shutdown: Option<tokio::sync::watch::Sender<bool>>) -> Router {
    // Hash the bridge bundle now, not on the first page served.
    shell::bridge_version();
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any);
    let publish_limit = DefaultBodyLimit::max(artifacts::PUBLISH_BODY_LIMIT);
    let asset_limit = DefaultBodyLimit::max(assets::UPLOAD_BODY_LIMIT);
    let ask_limit = DefaultBodyLimit::max(questions::ASK_BODY_LIMIT);
    let api_fast = Router::new()
        .route("/api/token", get(token::token))
        .route("/api/artifacts", get(artifacts::list))
        .route(
            "/api/sessions",
            get(sessions::list).post(sessions::register),
        )
        .route("/api/sessions/join", post(sessions::join))
        .route("/api/push", get(sessions::push_status))
        .route("/api/sample", get(sample::daemon))
        .route(
            "/api/sessions/{id}",
            get(sessions::get).patch(sessions::patch),
        )
        .route(
            "/api/artifacts/{aid}",
            get(artifacts::get)
                .patch(artifacts::patch)
                .delete(artifacts::delete),
        )
        .route(
            "/api/artifacts/{aid}/versions",
            get(artifacts::list_versions),
        )
        .route(
            "/api/artifacts/{aid}/versions/{n}",
            get(artifacts::get_version),
        )
        .route("/api/artifacts/{aid}/files", get(artifacts::files))
        .route("/api/artifacts/{aid}/assets", get(assets::list))
        .route(
            "/api/artifacts/{aid}/assets/{asset_id}",
            delete(assets::delete),
        )
        .route("/api/threads", get(threads::list_all))
        .route("/api/working", get(working::roster))
        .route("/api/artifacts/{aid}/threads", get(threads::list))
        .route(
            "/api/artifacts/{aid}/threads/{tid}",
            get(threads::get).delete(threads::delete),
        )
        .route(
            "/api/artifacts/{aid}/threads/{tid}/clip",
            get(threads::clip),
        )
        .route(
            "/api/artifacts/{aid}/threads/{tid}/comments",
            post(threads::comment),
        )
        .route(
            "/api/artifacts/{aid}/threads/{tid}/send",
            post(threads::send),
        )
        .route(
            "/api/artifacts/{aid}/threads:send",
            post(threads::send_batch),
        )
        .route(
            "/api/artifacts/{aid}/approves-first",
            get(threads::approves_first),
        )
        .route(
            "/api/artifacts/{aid}/threads/{tid}/resolve",
            post(threads::resolve),
        )
        .route(
            "/api/artifacts/{aid}/threads/{tid}/reopen",
            post(threads::reopen),
        )
        .route("/api/viewers", get(viewers::lookup))
        .route("/api/live/pages", get(live::page))
        .route("/api/live/site", get(live::site))
        .route("/api/live/rules", get(live::rules))
        .route("/api/live/rules/{id}", delete(live::delete_rule))
        .route("/api/live/sites", get(sites::list))
        .route("/api/live/sites/suggest", get(sites::suggest))
        .route("/api/live/sites/split", post(sites::split))
        .route("/api/live/sites/answer", post(sites::answer))
        .route("/api/extension", get(extension::status))
        .route("/api/toolpath/status", get(toolpath::status))
        .route(
            "/api/extension/credentials",
            post(extension::mint).delete(extension::revoke),
        )
        .route("/api/stream/{id}", post(stream::update))
        .route(
            "/api/viewers/me/seen",
            get(viewers::seen).put(viewers::set_seen),
        )
        .route("/api/viewers/me/attention", get(viewers::attention))
        .route(
            "/api/viewers/me/looked",
            axum::routing::put(viewers::set_looked),
        )
        .route(
            "/api/viewers/me/presence",
            axum::routing::put(viewers::set_presence),
        )
        .route("/api/viewers/me", get(viewers::me).put(viewers::set_me))
        .route("/api/sessions/{id}/watches", get(watches::list))
        .route(
            "/api/sessions/{id}/live-watches",
            axum::routing::put(watches::live_put).delete(watches::live_delete),
        )
        .route(
            "/api/sessions/{id}/watches/{aid}",
            axum::routing::put(watches::put).delete(watches::delete),
        )
        .route("/api/sessions/{id}/feedback/ack", post(feedback::ack))
        .route(
            "/api/sessions/{id}/tool-calls",
            post(toolpath::session_tool_call),
        )
        .route("/api/tool-calls", post(toolpath::tool_call))
        .route(
            "/api/sessions/{id}/tool-call-ids",
            post(sessions::tool_call_ids),
        )
        .route(
            "/api/sessions/{id}/questions",
            post(questions::create.layer(ask_limit)),
        )
        .route(
            "/api/sessions/{id}/questions:terminal",
            post(questions::terminal),
        )
        .route(
            "/api/sessions/{id}/questions/{qid}/withdraw",
            post(questions::withdraw),
        )
        .route(
            "/api/sessions/{id}/questions/{qid}/release",
            post(questions::release),
        )
        .route("/api/questions", get(questions::list))
        .route("/api/questions/{qid}", get(questions::get_one))
        .route("/api/questions/{qid}/answer", post(questions::answer))
        .route("/api/questions/{qid}/decline", post(questions::decline))
        .route(
            "/api/questions/{qid}/release",
            post(questions::release_owner),
        )
        .route("/api/sessions/{id}/approvals", post(approvals::ask))
        .route(
            "/api/sessions/{id}/threads/{tid}/handoff",
            post(threads::handoff),
        )
        .route(
            "/api/approvals",
            get(approvals::list).post(approvals::create),
        )
        .route("/api/approvals/{id}", delete(approvals::revoke))
        .route("/api/approvals/requests", get(approvals::requests))
        .route(
            "/api/approvals/requests/{rid}/decide",
            post(approvals::decide),
        )
        .route("/api/inbox", get(inbox::list))
        .route("/api/inbox/summary", get(inbox::summary))
        .route("/api/inbox/read", post(inbox::read_many))
        .route("/api/inbox/{id}", get(inbox::get_one))
        .route("/api/inbox/{id}/read", post(inbox::read_one))
        .route("/api/inbox/{id}/unread", post(inbox::unread_one))
        .route("/api/artifacts/{aid}/docs", get(docs::list))
        .route(
            "/api/artifacts/{aid}/docs/{*path}",
            get(docs::get)
                .put(docs::put.layer(DefaultBodyLimit::max(
                    <crate::agent::DocsPut as crate::agent::Need>::LIMIT,
                )))
                .patch(docs::patch)
                .delete(docs::delete),
        )
        .route(
            "/api/artifacts/{aid}/docs:batch",
            post(docs::batch.layer(DefaultBodyLimit::max(docs::DOCS_BATCH_LIMIT))),
        )
        .route(
            "/api/artifacts/{aid}/docs:str_replace",
            post(docs::str_replace),
        )
        .route("/api/artifacts/{aid}/docs:acquire", post(docs::acquire))
        .route("/api/artifacts/{aid}/working", get(working::for_artifact))
        .route("/api/artifacts/{aid}/presence", get(artifacts::presence))
        .route("/api/artifacts/{aid}/snapshot", get(snapshot::snapshot))
        .route("/api/admin/perf/calibrate", post(perf::calibrate))
        .route("/api/sessions/{id}/working", get(working::for_session))
        .route("/api/sessions/{id}/working/renew", post(working::renew))
        .route("/api/sessions/{id}/working/end", post(working::end))
        .route(
            "/api/sessions/{id}/working/{aid}",
            axum::routing::put(working::put).delete(working::delete),
        )
        .route("/api/grants", get(grants::list).post(grants::request))
        .route("/api/grants/{gid}", delete(grants::revoke))
        .route(
            "/api/grants/{gid}/continue",
            post(grants::poll).delete(grants::disconnect),
        )
        .route("/api/grants/{gid}/token", delete(grants::revoke_token))
        .route("/api/grants/{gid}/approve", post(grants::approve))
        .route("/api/grants/{gid}/deny", post(grants::deny))
        .route(
            "/api/people/invites",
            get(grants::list_invites).post(grants::create_invite),
        )
        .route("/api/people/invites/{id}", delete(grants::revoke_invite))
        .route("/api/people", get(people::list_people))
        .route("/api/people/me", get(people::me))
        .route(
            "/api/people/me/grants/{gid}/narrow",
            axum::routing::put(people::narrow_grant),
        )
        .route("/api/people/{pid}", delete(people::revoke_person))
        .route(
            "/api/people/{pid}/rights",
            axum::routing::put(people::set_rights),
        )
        .route("/api/agents", get(people::list_agents))
        .route(
            "/api/agents/{handle}",
            axum::routing::put(people::rename_agent).delete(people::end_agent),
        )
        .route(
            "/api/agents/{handle}/narrow",
            axum::routing::put(people::narrow_agent),
        )
        .route(
            "/api/agents/{handle}/elevate",
            axum::routing::put(people::elevate_agent).delete(people::unelevate_agent),
        );
    #[cfg(feature = "test-routes")]
    let api_fast = api_fast
        .route("/api/_test/sleep/{ms}", get(test_sleep))
        .route("/api/_test/slow_publish/{ms}", post(test_slow_publish))
        .route("/api/_test/audit/ctx", get(test_audit_ctx));
    #[cfg(debug_assertions)]
    let api_fast = api_fast
        .route("/api/_test/working/skew", post(working::skew))
        .route("/api/_test/stream/open", get(stream::open_streams))
        .route(
            "/api/_test/questions/{qid}/waiters",
            get(questions::waiters),
        )
        .route("/api/_test/questions/{qid}/expire", post(questions::expire))
        .route(
            "/api/_test/sessions/{id}/feedback/waiters",
            get(feedback::waiters),
        )
        .route(
            "/api/_test/sessions/{id}/feedback/expire",
            post(feedback::expire),
        )
        .route(
            "/api/_test/sessions/{id}/feedback/hold",
            post(feedback::hold),
        )
        // The same, at paths that name no session for the caller, so a
        // test reaches a grant's session with the token.
        .route("/api/_test/feedback/{id}/waiters", get(feedback::waiters))
        .route("/api/_test/feedback/{id}/expire", post(feedback::expire))
        .route("/api/_test/feedback/{id}/hold", post(feedback::hold))
        .route("/api/_test/grants/sweep", post(grants::test_sweep))
        .route("/api/_test/agent/gate", get(crate::agent::test_gate))
        .route("/api/_test/caller", get(crate::agent::test_caller))
        .route(
            "/api/_test/sessions/{id}/caller",
            get(crate::agent::test_caller),
        )
        .route("/api/_test/grants/index", get(grants::test_index))
        .route(
            "/api/_test/grants/index/fail",
            post(grants::test_fail_index),
        );
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    let api_fast = api_fast.route("/api/_test/events/open", get(events::open_streams));
    #[cfg(feature = "test-routes")]
    let api_fast = api_fast.layer(axum::middleware::from_fn(test_delay));
    let api_fast = api_fast.layer(axum::middleware::from_fn(crate::http_cache::api_etag));
    let api_fast =
        with_timeout(api_fast, state.request_timeout).layer(crate::http_cache::compression());
    let api_slow = Router::new()
        .route(
            "/api/artifacts",
            post(artifacts::create.layer(publish_limit)),
        )
        .route(
            "/api/artifacts/{aid}/versions",
            post(artifacts::publish.layer(publish_limit)),
        )
        .route(
            "/api/artifacts/{aid}/assets",
            post(assets::upload.layer(asset_limit)),
        )
        .route(
            "/api/artifacts/{aid}/threads",
            post(threads::create.layer(DefaultBodyLimit::max(threads::THREAD_BODY_LIMIT))),
        )
        .route(
            "/api/live/threads",
            post(live::thread.layer(DefaultBodyLimit::max(live::LIVE_THREAD_LIMIT))),
        )
        .route(
            "/api/live/snapshots",
            post(live::snapshot.layer(DefaultBodyLimit::max(live::LIVE_THREAD_LIMIT))),
        )
        // Moves and merges copy snapshot versions.
        .route("/api/live/threads/{tid}/move", post(live::move_thread))
        .route("/api/live/rules", post(live::add_rule))
        // A join merges pages as moves do.
        .route("/api/live/sites/join", post(sites::join));
    #[cfg(feature = "test-routes")]
    let api_slow = api_slow.layer(axum::middleware::from_fn(test_delay));
    let api_slow =
        with_timeout(api_slow, state.publish_timeout).layer(crate::http_cache::compression());
    let shell_routes = Router::new()
        .route("/", get(shell::gallery_page))
        .route("/inbox", get(shell::gallery_page))
        .route("/people", get(shell::gallery_page))
        .route("/a/{aid}", get(shell::artifact_page))
        .route("/a/{aid}/", get(shell::artifact_page))
        .route("/a/{aid}/v/{n}", get(shell::artifact_page))
        // `/a/<id>[/v/<n>]/<file>`: the shell reads the version and page from the path.
        .route("/a/{aid}/{*rest}", get(shell::artifact_page))
        .route("/_clax/{*path}", get(shell::static_file))
        .layer(crate::http_cache::compression());
    let r = Router::new()
        .route("/healthz", get(health::healthz).layer(cors))
        .route(
            "/api/artifacts/{aid}/sample",
            get(sample::status)
                .post(sample::sample.layer(DefaultBodyLimit::max(sample::SAMPLE_BODY_LIMIT))),
        )
        .route(
            "/api/artifacts/{aid}/sample/{call}/tool_result",
            post(sample::tool_result),
        )
        .route("/api/sessions/{id}/feedback", get(feedback::poll))
        .route("/api/sessions/{id}/notices", get(feedback::notices))
        // A long-poll: outside the request timeout, as the feedback poll.
        .route("/api/sessions/{id}/questions/{qid}", get(questions::poll))
        .route("/api/sessions/{id}/approvals/{rid}", get(approvals::poll))
        .merge(api_fast)
        .merge(api_slow)
        .merge(shell_routes)
        .route("/_blob/{asset_id}", get(assets::blob))
        .route("/connect/b/{code}", get(connect::browser_link))
        .route("/c/{aid}/v/{n}", get(content::redirect_to_slash))
        .route("/c/{aid}/v/{n}/", get(content::index))
        .route("/c/{aid}/v/{n}/{*path}", get(content::file))
        .route(
            "/api/artifacts/{aid}/versions/{n}/files/{*path}",
            get(content::raw_file),
        );
    // What holds its connection open: the daemon's. A Durable Object
    // carries the streams and rooms over its own hibernatable sockets and
    // serves `/mcp` with the tool set's JSON-RPC adapter (see the crate
    // documentation).
    #[cfg(not(target_arch = "wasm32"))]
    let r = {
        let mcp = mcp::router(&state);
        #[cfg(feature = "test-routes")]
        let mcp = mcp.layer(axum::middleware::from_fn(test_delay));
        // Streams for as long as its reader takes: no request deadline, no
        // buffering ETag.
        r.route("/api/toolpath/export", get(toolpath::export))
            .route("/api/events", get(events::events))
            .route("/api/stream", get(stream::open))
            .route("/api/artifacts/{aid}/room", get(room::room))
            .merge(mcp)
    };
    let mut r = r;
    if let Some(tx) = shutdown {
        let tx = std::sync::Arc::new(tx);
        r = r.route(
            "/api/admin/shutdown",
            post(move |_d: crate::agent::DeniedToGrants, _t: RequireToken| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(true);
                    StatusCode::ACCEPTED
                }
            }),
        );
    }
    // An agent of another machine's request is verified before any handler
    // runs, on every route (spec §6.6): a route layer, so it sees the route
    // matched, over every route above.
    let r = r.route_layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::agent::verify_layer,
    ));
    // Live pages answer 404 to callers that may not see them, before any
    // handler runs (spec 2026-10-05-chrome-overlay-design L10).
    // The extension gateway runs first (the last layer is outermost): only
    // what it admits from the extension's origin reaches the rest (spec
    // 2026-10-05-chrome-overlay-design L5).
    r.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::live::hide_live_pages,
    ))
    .layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::extension::gateway,
    ))
    .with_state(state)
}

/// Sleeps for the milliseconds in the `x-clax-test-delay-ms` request
/// header, if any, before handling the request. It sits inside each route
/// group's timeout layer, so tests can make a real route slow.
#[cfg(feature = "test-routes")]
async fn test_delay(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let ms = req
        .headers()
        .get("x-clax-test-delay-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if let Some(ms) = ms {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    next.run(req).await
}

/// The audit context the request resolves, as JSON: `actor`, `via`, and
/// `git`, `git_capture` and `call` (null when absent).
#[cfg(feature = "test-routes")]
async fn test_audit_ctx(ctx: clax_core::audit::AuditCtx) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "actor": ctx.actor,
        "via": ctx.via,
        "git": ctx.git.context(),
        "git_capture": ctx.git.capture(),
        "call": ctx.call,
    }))
}

#[cfg(feature = "test-routes")]
async fn test_sleep(
    _d: crate::agent::DeniedToGrants,
    axum::extract::Path(ms): axum::extract::Path<u64>,
) -> StatusCode {
    tokio::time::sleep(Duration::from_millis(ms)).await;
    StatusCode::OK
}

/// Creates an artifact after sleeping inside the blocking closure, publishing the event from the
/// same closure, so tests can observe write side effects surviving a handler timeout.
#[cfg(feature = "test-routes")]
async fn test_slow_publish(
    axum::extract::State(s): axum::extract::State<AppState>,
    _d: crate::agent::DeniedToGrants,
    axum::extract::Path(ms): axum::extract::Path<u64>,
) -> Result<StatusCode, ApiError> {
    let events = s.events.clone();
    s.store_call(move |st| {
        std::thread::sleep(Duration::from_millis(ms));
        let p = clax_core::publish::validate(
            serde_json::from_value(serde_json::json!({
                "title": "slow",
                "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
            }))
            .expect("valid publish request"),
        )?;
        let (artifact, version) =
            st.create_artifact(&clax_core::audit::AuditCtx::DAEMON, p, None)?;
        events.publish(clax_core::Event::Version {
            artifact_id: artifact.id,
            n: version.n,
            by_page: false,
            title: Some(artifact.title.clone()),
            at: Some(version.created_at.clone()),
        });
        Ok(())
    })
    .await?;
    Ok(StatusCode::CREATED)
}

/// Every route keeps live pages from callers that may not see them (spec
/// 2026-10-05-chrome-overlay-design L10). The routes are read from this
/// module's source, so a new one fails here until it is path-covered (an
/// artifact ID in the path that `hide_live_pages` reads, or `/api/live/…`)
/// or its handler is listed in [`l10::GUARDS`] with the guard its source
/// shows.
#[cfg(test)]
mod l10 {
    /// How a handler of a route outside the path rule keeps live pages hidden.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Guard {
        /// It takes `RequireToken`: the token sees live pages, and an agent
        /// of another machine never gets past it.
        Token,
        /// It takes `Agent<…>` or `TokenOrAgent<…>`, and reaches an artifact
        /// only through the extractor: `.artifact()` (its path's `{aid}`)
        /// or `.named_artifact(…)` (one its query or body names), never
        /// parsing an ID itself. The token sees live pages; the extractor
        /// answers an agent of another machine 404 for a live page either
        /// way (`hide_live_pages` does not cover `/api/sessions/…`).
        AgentPath,
        /// It takes `Agent<…>` or `TokenOrAgent<…>` whose need keeps a
        /// person's agent off the route (`agent::ROUTES`: `grants` false,
        /// or a `Filtered` need not yet in `FILTERED_LIVE`): the token sees
        /// live pages, and no agent of another machine reaches it. A task
        /// that opens it to grants must give it another guard.
        GrantsKept,
        /// It takes `SeesLive` and checks the artifact its query or body names.
        SeesLive,
        /// A listing or stream that leaves live pages out unless
        /// `sees_live_pages`.
        Filtered,
        /// It takes `Agent<…>` or `TokenOrAgent<…>` and makes a new
        /// artifact: it names none that exists, so neither parses one from
        /// the request nor names one outside its path.
        Creates,
        /// It names no artifact.
        NoArtifact,
        /// It refuses every caller but the owner (`who.require_owner(…)`, or
        /// the audit history's narrower token-or-owner-cookie check); every
        /// owner credential sees live pages (the extension's is
        /// loopback-only).
        Owner,
        /// It serves one person's own items, the owner's or a connected
        /// person's (`recipient_of(&s, &who)`, which reads `who.person(`),
        /// and hands that recipient to every store read and mark, never an
        /// unscoped lookup: no item about a live page is made for a person
        /// (clax-core's inbox), and a person's view names none
        /// ([`crate::inbox`]), so a LAN browser never learns of one.
        Person,
    }
    use Guard::*;

    /// The handlers of the routes the path rule does not cover.
    const GUARDS: &[(&str, Guard)] = &[
        ("token::token", NoArtifact),
        ("artifacts::list", Filtered),
        ("artifacts::create", Creates),
        ("sessions::list", AgentPath),
        ("sessions::register", AgentPath),
        ("sessions::join", AgentPath),
        ("sessions::get", AgentPath),
        ("sessions::patch", AgentPath),
        ("sessions::push_status", NoArtifact),
        ("sessions::tool_call_ids", NoArtifact),
        ("sample::daemon", Token),
        ("threads::list_all", GrantsKept),
        ("working::roster", Filtered),
        ("working::for_session", AgentPath),
        ("working::renew", AgentPath),
        ("working::end", AgentPath),
        ("working::put", AgentPath),
        ("working::delete", AgentPath),
        ("working::skew", Token),
        ("viewers::lookup", NoArtifact),
        ("viewers::me", NoArtifact),
        ("viewers::set_me", NoArtifact),
        ("viewers::seen", SeesLive),
        ("viewers::set_seen", SeesLive),
        ("viewers::attention", SeesLive),
        ("viewers::set_looked", SeesLive),
        ("viewers::set_presence", SeesLive),
        ("extension::status", Token),
        ("extension::mint", Token),
        ("extension::revoke", Token),
        ("stream::open", Filtered),
        ("stream::update", SeesLive),
        ("stream::open_streams", Token),
        ("events::events", Filtered),
        ("events::open_streams", Token),
        ("watches::list", AgentPath),
        ("watches::put", AgentPath),
        ("watches::delete", AgentPath),
        ("watches::live_put", Token),
        ("watches::live_delete", Token),
        ("feedback::ack", AgentPath),
        ("toolpath::session_tool_call", AgentPath),
        ("toolpath::tool_call", Token),
        ("feedback::poll", AgentPath),
        ("feedback::notices", AgentPath),
        ("questions::create", AgentPath),
        ("questions::poll", AgentPath),
        ("questions::withdraw", AgentPath),
        ("questions::release", AgentPath),
        ("questions::terminal", AgentPath),
        ("questions::waiters", Token),
        ("feedback::waiters", Token),
        ("questions::expire", Token),
        ("feedback::expire", Token),
        ("feedback::hold", Token),
        ("approvals::ask", AgentPath),
        ("threads::handoff", AgentPath),
        ("approvals::poll", AgentPath),
        ("approvals::list", NoArtifact),
        ("approvals::requests", NoArtifact),
        ("approvals::create", NoArtifact),
        ("approvals::revoke", NoArtifact),
        ("approvals::decide", NoArtifact),
        ("questions::list", Person),
        ("questions::get_one", Person),
        ("questions::answer", Person),
        ("questions::decline", Person),
        ("questions::release_owner", Person),
        ("inbox::list", Person),
        ("inbox::summary", Person),
        ("inbox::read_many", Person),
        ("inbox::get_one", Person),
        ("inbox::read_one", Person),
        ("inbox::unread_one", Person),
        ("shell::gallery_page", NoArtifact),
        ("shell::static_file", NoArtifact),
        ("health::healthz", NoArtifact),
        ("assets::blob", SeesLive),
        ("perf::calibrate", Token),
        ("toolpath::export", Owner),
        ("toolpath::status", Owner),
        ("grants::list", Owner),
        ("grants::request", NoArtifact),
        ("grants::revoke", NoArtifact),
        ("grants::poll", NoArtifact),
        ("grants::disconnect", NoArtifact),
        ("grants::revoke_token", NoArtifact),
        ("grants::approve", NoArtifact),
        ("grants::deny", NoArtifact),
        ("grants::list_invites", Owner),
        ("grants::create_invite", NoArtifact),
        ("grants::revoke_invite", NoArtifact),
        ("grants::test_sweep", Token),
        ("grants::test_index", Token),
        ("grants::test_fail_index", Token),
        ("connect::browser_link", NoArtifact),
        ("people::rename_agent", NoArtifact),
        ("people::end_agent", NoArtifact),
        ("people::narrow_agent", NoArtifact),
        ("people::elevate_agent", NoArtifact),
        ("people::unelevate_agent", NoArtifact),
        ("people::list_people", NoArtifact),
        ("people::me", NoArtifact),
        ("people::narrow_grant", NoArtifact),
        ("people::revoke_person", NoArtifact),
        ("people::set_rights", NoArtifact),
        ("people::list_agents", Filtered),
        ("agent::test_gate", Token),
        ("agent::test_caller", NoArtifact),
    ];

    /// Routes whose handler is not a module function, and why they are safe.
    const INLINE: &[(&str, &str)] = &[
        (
            "/api/_test/sleep/{ms}",
            "test-routes only; names no artifact",
        ),
        (
            "/api/_test/slow_publish/{ms}",
            "test-routes only; makes an HTML artifact",
        ),
        (
            "/api/_test/audit/ctx",
            "test-routes only; names no artifact",
        ),
        ("/api/admin/shutdown", "RequireToken"),
    ];

    pub(super) fn module_source(m: &str) -> &'static str {
        module_source_opt(m).unwrap_or_else(|| panic!("no route module {m}"))
    }

    pub(super) fn module_source_opt(m: &str) -> Option<&'static str> {
        Some(match m {
            "approvals" => include_str!("approvals.rs"),
            "artifacts" => include_str!("artifacts.rs"),
            "assets" => include_str!("assets.rs"),
            "connect" => include_str!("connect.rs"),
            "grants" => include_str!("grants.rs"),
            "content" => include_str!("content.rs"),
            "docs" => include_str!("docs.rs"),
            "events" => include_str!("events.rs"),
            "extension" => include_str!("extension.rs"),
            "feedback" => include_str!("feedback.rs"),
            "health" => include_str!("health.rs"),
            "inbox" => include_str!("inbox.rs"),
            "live" => include_str!("live.rs"),
            "perf" => include_str!("perf.rs"),
            "questions" => include_str!("questions.rs"),
            "room" => include_str!("room.rs"),
            "sample" => include_str!("sample.rs"),
            "sessions" => include_str!("sessions.rs"),
            "shell" => include_str!("shell.rs"),
            "sites" => include_str!("sites.rs"),
            "snapshot" => include_str!("snapshot.rs"),
            "stream" => include_str!("stream.rs"),
            "threads" => include_str!("threads.rs"),
            "token" => include_str!("token.rs"),
            "toolpath" => include_str!("toolpath.rs"),
            "viewers" => include_str!("viewers.rs"),
            "watches" => include_str!("watches.rs"),
            "working" => include_str!("working.rs"),
            "agent" => include_str!("../agent.rs"),
            "mcp" => include_str!("mcp.rs"),
            "people" => include_str!("people.rs"),
            _ => return None,
        })
    }

    /// Each `.route("<path>", <handlers>)` of `src`: the path and the
    /// handlers' text.
    pub(super) fn routes(src: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find(".route(") {
            let after = &rest[i + ".route(".len()..];
            let mut depth = 1;
            let mut end = 0;
            for (j, c) in after.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = j;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let call = &after[..end];
            let open = call.find('"').expect("a path literal");
            let close = open + 1 + call[open + 1..].find('"').unwrap();
            out.push((
                call[open + 1..close].to_string(),
                call[close + 1..].to_string(),
            ));
            rest = &after[end..];
        }
        out
    }

    /// The `module::function` handlers named in `text`.
    pub(super) fn handlers(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let b = text.as_bytes();
        let ident = |c: u8| c.is_ascii_lowercase() || c == b'_';
        let mut i = 0;
        while let Some(k) = text[i..].find("::") {
            let at = i + k;
            let mut s = at;
            while s > 0 && ident(b[s - 1]) {
                s -= 1;
            }
            let mut e = at + 2;
            while e < b.len() && ident(b[e]) {
                e += 1;
            }
            let (m, f) = (&text[s..at], &text[at + 2..e]);
            if !m.is_empty() && !f.is_empty() && !["axum", "routing", "crate"].contains(&m) {
                out.push((m.to_string(), f.to_string()));
            }
            i = at + 2;
        }
        out
    }

    /// The text of `pub async fn name` in `src`: its signature and body.
    pub(super) fn function<'a>(src: &'a str, name: &str) -> &'a str {
        let start = src
            .find(&format!("pub async fn {name}("))
            .unwrap_or_else(|| panic!("no handler {name}"));
        let rest = &src[start..];
        let end = rest.find("\n}\n").map_or(rest.len(), |e| e + 3);
        &rest[..end]
    }

    /// Resolving an [`AuditCtx`](clax_core::audit::AuditCtx) makes the
    /// owner's or a first-time viewer's row when it is missing, so only
    /// handlers that record events take it (or a
    /// [`DeferredAudit`](crate::audit::DeferredAudit) to resolve it later),
    /// and no `GET` handler does.
    #[test]
    fn no_get_route_resolves_the_audit_context() {
        let src = include_str!("mod.rs");
        let src = &src[..src.find("mod l10 {").unwrap()];
        let ident = |c: char| c.is_ascii_lowercase() || c == '_' || c == ':';
        let mut checked = 0;
        for (path, text) in routes(src) {
            let mut rest = text.as_str();
            while let Some(i) = rest.find("get(") {
                let before = rest[..i].chars().next_back();
                let after = &rest[i + "get(".len()..];
                rest = after;
                if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                    continue;
                }
                let name: String = after.chars().take_while(|c| ident(*c)).collect();
                let Some((m, f)) = name.trim_start_matches("crate::").split_once("::") else {
                    continue;
                };
                let sig = function(module_source(m), f);
                let sig = &sig[..sig.find('{').unwrap()];
                assert!(
                    !sig.contains("AuditCtx") && !sig.contains("DeferredAudit"),
                    "GET {path} ({name}) takes AuditCtx or DeferredAudit"
                );
                checked += 1;
            }
        }
        assert!(checked > 20, "found only {checked} GET handlers");
    }

    /// The handlers that take [`AuditCtx`](clax_core::audit::AuditCtx) and
    /// record some of their changes (the pages and snapshots they make) but
    /// not yet their main event: the kinds each still owes, and the plan task
    /// (plan 2026-10-06-toolpath-audit) that records them. Remove an entry
    /// with the change that records its kinds.
    const PARTLY_AUDITED: &[(&str, &str)] = &[];

    /// The handlers of routes that are not `GET` and take no audit context
    /// ([`AuditCtx`](clax_core::audit::AuditCtx) or
    /// [`DeferredAudit`](crate::audit::DeferredAudit)), and why.
    const UNAUDITED: &[(&str, &str)] = &[
        ("working::renew", "a heartbeat (§6.11)"),
        (
            "working::skew",
            "debug builds only; shifts the working clock (the lapses it causes are recorded by the sweep as system:ttl)",
        ),
        ("extension::mint", "an extension credential grant (§6.11)"),
        ("extension::revoke", "an extension credential grant (§6.11)"),
        ("stream::update", "a stream subscription (§6.11)"),
        ("viewers::set_presence", "presence (§6.11)"),
        ("viewers::set_seen", "a viewer's read marks, not history"),
        ("viewers::set_looked", "a viewer's read marks, not history"),
        ("viewers::set_me", "a viewer's own name, not history"),
        (
            "sample::sample",
            "a page's model request; changes no history",
        ),
        (
            "sample::tool_result",
            "a page's model request; changes no history",
        ),
        (
            "perf::calibrate",
            "the latency gate's calibration read; changes no history",
        ),
        (
            "questions::expire",
            "debug builds only; ends a question's polls, changing no history",
        ),
        (
            "feedback::expire",
            "debug builds only; ends a session's feedback polls, changing no history",
        ),
        (
            "feedback::hold",
            "debug builds only; stops a session's feedback polls after their take, changing no history",
        ),
        (
            "inbox::read_many",
            "the owner's inbox read marks, not history (§6.11)",
        ),
        (
            "inbox::read_one",
            "the owner's inbox read marks, not history (§6.11)",
        ),
        (
            "inbox::unread_one",
            "the owner's inbox read marks, not history (§6.11)",
        ),
        (
            "grants::test_fail_index",
            "debug builds only; makes the token index's next change fail",
        ),
        (
            "grants::test_sweep",
            "debug builds only; runs the daemon's grant sweep, which records its lapses as system:expiry",
        ),
    ];

    /// Every handler of a route that is not `GET` either resolves the audit
    /// context, during extraction or deferred (and records what it changes, or is listed in
    /// [`PARTLY_AUDITED`] with the kinds it still owes) or is listed in
    /// [`UNAUDITED`] with the reason it records nothing, so a new route that
    /// changes history cannot leave it out of the record unnoticed.
    #[test]
    fn every_change_route_resolves_the_audit_context_or_says_why_not() {
        let src = include_str!("mod.rs");
        let src = &src[..src.find("mod l10 {").unwrap()];
        let ident = |c: char| c.is_ascii_lowercase() || c == '_' || c == ':';
        let mut used = std::collections::HashSet::new();
        let mut partly_used = std::collections::HashSet::new();
        let mut missing = Vec::new();
        let mut checked = 0;
        for (path, text) in routes(src) {
            for verb in ["post(", "put(", "patch(", "delete("] {
                let mut rest = text.as_str();
                while let Some(i) = rest.find(verb) {
                    let before = rest[..i].chars().next_back();
                    let after = &rest[i + verb.len()..];
                    rest = after;
                    if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                        continue;
                    }
                    let name: String = after.chars().take_while(|c| ident(*c)).collect();
                    let Some((m, f)) = name.trim_start_matches("crate::").split_once("::") else {
                        // An inline handler: INLINE says what it is.
                        continue;
                    };
                    let sig = function(module_source(m), f);
                    let sig = &sig[..sig.find('{').unwrap()];
                    let listed = UNAUDITED.iter().find(|(h, _)| *h == name);
                    let partly = PARTLY_AUDITED.iter().any(|(h, _)| *h == name);
                    if sig.contains("AuditCtx") || sig.contains("DeferredAudit") {
                        assert!(
                            listed.is_none(),
                            "{verb}{path}) ({name}) takes AuditCtx: drop it from UNAUDITED"
                        );
                        if partly {
                            partly_used.insert(name.clone());
                        }
                    } else if partly {
                        panic!(
                            "{verb}{path}) ({name}) takes no AuditCtx but PARTLY_AUDITED lists it"
                        );
                    } else if listed.is_some() {
                        used.insert(name.clone());
                    } else {
                        missing.push(format!("{verb}{path}) ({name})"));
                    }
                    checked += 1;
                }
            }
        }
        assert!(
            missing.is_empty(),
            "these take no AuditCtx: take it, or list them in UNAUDITED with the reason:\n{}",
            missing.join("\n")
        );
        assert!(checked > 30, "found only {checked} handlers");
        for (h, _) in UNAUDITED {
            assert!(
                used.contains(*h),
                "UNAUDITED lists {h}, which is no such handler"
            );
        }
        for (h, _) in PARTLY_AUDITED {
            assert!(
                partly_used.contains(*h),
                "PARTLY_AUDITED lists {h}, which is no such handler"
            );
        }
    }

    /// Each `method(module::function` of a route's handlers' text.
    pub(super) fn method_handlers(text: &str) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        let b = text.as_bytes();
        for m in ["get", "post", "put", "patch", "delete"] {
            let pat = format!("{m}(");
            let mut i = 0;
            while let Some(k) = text[i..].find(&pat) {
                let at = i + k;
                i = at + pat.len();
                if at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_') {
                    continue;
                }
                let rest = &text[i..];
                let end = rest
                    .find(|c: char| !(c.is_ascii_lowercase() || c == '_' || c == ':'))
                    .unwrap_or(rest.len());
                if let Some((module, f)) = rest[..end].split_once("::")
                    && !module.is_empty()
                    && !f.is_empty()
                    && !f.contains(':')
                {
                    out.push((m.to_uppercase(), module.to_string(), f.to_string()));
                }
            }
        }
        out
    }

    /// The declared need of the handler `m::f` routed at `path` in `text`.
    pub(super) fn need_of_handler(
        path: &str,
        text: &str,
        m: &str,
        f: &str,
    ) -> Option<crate::agent::NeedInfo> {
        method_handlers(text)
            .into_iter()
            .find(|(_, hm, hf)| hm == m && hf == f)
            .and_then(|(method, _, _)| crate::agent::need_of(&method.parse().unwrap(), path))
    }

    /// The need an `Agent<…>` or `TokenOrAgent<…>` in `sig` names.
    fn agent_need(sig: &str) -> Option<String> {
        let at = sig.find("Agent<")? + "Agent<".len();
        let rest = &sig[at..];
        let name = &rest[..rest.find('>')?];
        Some(name.rsplit("::").next().unwrap().to_string())
    }

    /// Every route an agent of another machine reaches is declared in
    /// `agent::ROUTES` under the need its handler takes, and every
    /// declaration is a route's: `verify_layer` finds a route's need by
    /// method and path, and a handler without `Agent<…>` is never in it.
    #[test]
    fn every_agent_route_is_declared_for_its_handler() {
        let src = include_str!("mod.rs");
        let src = &src[..src.find("mod l10 {").unwrap()];
        let mut seen = std::collections::HashSet::new();
        for (path, text) in routes(src) {
            for (method, m, f) in method_handlers(&text) {
                if module_source_opt(&m).is_none() {
                    continue;
                }
                let fsrc = function(module_source(&m), &f);
                let sig = &fsrc[..fsrc.find('{').unwrap()];
                let declared = crate::agent::ROUTES
                    .iter()
                    .find(|(rm, rp, _)| *rm == method && *rp == path);
                match (agent_need(sig), declared) {
                    (Some(n), Some((_, _, info))) => {
                        assert_eq!(info.name, n, "{method} {path}: {m}::{f} takes Agent<{n}>");
                        seen.insert((method.clone(), path.clone()));
                    }
                    (Some(n), None) => panic!(
                        "{method} {path}: {m}::{f} takes Agent<{n}> but agent::ROUTES lacks it"
                    ),
                    (None, Some(_)) => panic!(
                        "agent::ROUTES declares {method} {path}, whose handler {m}::{f} takes no Agent"
                    ),
                    (None, None) => {}
                }
            }
        }
        for (m, p, _) in crate::agent::ROUTES {
            assert!(
                seen.contains(&(m.to_string(), p.to_string())),
                "agent::ROUTES declares {m} {p}, which no handler serves"
            );
        }
    }

    /// `Agent` finds the session a route's path names by the parameter
    /// `{id}` under `/api/sessions/` (`agent::names_a_session`): a route
    /// that named it otherwise would skip the session check.
    #[test]
    fn every_session_route_names_its_session_id() {
        let src = include_str!("mod.rs");
        let src = &src[..src.find("mod l10 {").unwrap()];
        for (path, _) in routes(src) {
            for prefix in ["/api/sessions/{", "/api/_test/sessions/{"] {
                if let Some(rest) = path.strip_prefix(prefix) {
                    assert!(
                        rest.starts_with("id}"),
                        "{path} names its session otherwise than {{id}}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_route_keeps_live_pages_hidden() {
        let aid = "7q3k9mzx2b4t";
        let mut used = std::collections::HashSet::new();
        let mut inline_used = std::collections::HashSet::new();
        let src = include_str!("mod.rs");
        let src = &src[..src.find("mod l10 {").unwrap()];
        for (path, text) in routes(src) {
            let concrete = path.replace("{aid}", aid);
            if crate::live::live_route(&concrete)
                || crate::live::artifact_in(&concrete).as_deref() == Some(aid)
            {
                continue;
            }
            let hs = handlers(&text);
            if hs.is_empty() {
                assert!(
                    INLINE.iter().any(|(p, _)| *p == path),
                    "{path}: an inline handler outside the path rule; list it in INLINE"
                );
                inline_used.insert(path.clone());
                continue;
            }
            for (m, f) in hs {
                let name = format!("{m}::{f}");
                let guard = GUARDS
                    .iter()
                    .find(|(h, _)| *h == name)
                    .map(|(_, g)| *g)
                    .unwrap_or_else(|| {
                        panic!("{path} ({name}) is outside the path rule: list its guard in GUARDS")
                    });
                let src = function(module_source(&m), &f);
                let sig = &src[..src.find('{').unwrap()];
                match guard {
                    Token => assert!(sig.contains("RequireToken"), "{name} takes no RequireToken"),
                    AgentPath => {
                        assert!(sig.contains("Agent<"), "{name} takes no Agent");
                        for parse in ["parse_id(", "ArtifactId::parse(", "ArtifactId::new("] {
                            assert!(
                                !src.contains(parse),
                                "{name} parses an artifact ID itself ({parse}): reach it through \
                                 the extractor's .artifact() or .named_artifact()"
                            );
                        }
                        let agent = include_str!("../agent.rs");
                        let extractor = function(agent, "named_artifact");
                        assert!(
                            agent.contains("s.live_ids.contains(a.as_str())")
                                && extractor.contains("s.live_ids.contains(id.as_str())"),
                            "Agent no longer hides live pages"
                        );
                    }
                    GrantsKept => {
                        let need = need_of_handler(&path, &text, &m, &f)
                            .unwrap_or_else(|| panic!("{name} takes no declared Agent"));
                        assert!(
                            !need.grants
                                || (need.class == crate::agent::Class::Filtered
                                    && !crate::agent::FILTERED_LIVE.contains(&need.name)),
                            "{name} is open to a person's agent: give it another guard"
                        );
                    }
                    Creates => {
                        assert!(sig.contains("Agent<"), "{name} takes no Agent");
                        for named in ["parse_id(", "named_artifact(", ".artifact()"] {
                            assert!(!src.contains(named), "{name} names an artifact ({named})");
                        }
                    }
                    SeesLive => assert!(
                        sig.contains("SeesLive") && src.contains(".check("),
                        "{name} does not check SeesLive"
                    ),
                    Filtered => assert!(
                        src.contains("sees_live_pages"),
                        "{name} does not filter by sees_live_pages"
                    ),
                    Owner => assert!(
                        sig.contains("who: Identity")
                            && (src.contains("who.require_owner(")
                                || src.split_once('{').is_some_and(|(_, body)| body
                                    .trim_start()
                                    .starts_with("owner_reads_history(&who)?;"))),
                        "{name} does not refuse callers other than the owner"
                    ),
                    NoArtifact => assert!(
                        !src.contains("ArtifactId") && !src.contains("parse_id"),
                        "{name} names an artifact"
                    ),
                    Person => {
                        assert!(
                            sig.contains("who: Identity")
                                && src.contains("let me = recipient_of(&s, &who).await?;")
                                && !src.contains("require_owner("),
                            "{name} does not scope itself to the caller's person"
                        );
                        // Every read and mark is the recipient's.
                        for unscoped in [
                            "db.question(",
                            "inbox_unread(None",
                            ", None)?",
                            "list_questions(None",
                        ] {
                            assert!(
                                !src.contains(unscoped),
                                "{name} reads past the caller's person ({unscoped})"
                            );
                        }
                        assert!(
                            !src.contains("InboxQuery {") || src.contains("recipient: me"),
                            "{name} builds a query of no one's inbox"
                        );
                        let inbox = include_str!("../inbox.rs");
                        assert!(
                            function(inbox, "recipient_of").contains("who.person(st)"),
                            "recipient_of no longer reads who.person"
                        );
                    }
                }
                used.insert(name);
            }
        }
        for (h, _) in GUARDS {
            assert!(used.contains(*h), "GUARDS lists {h}, which no route uses");
        }
        for (p, _) in INLINE {
            assert!(
                inline_used.contains(*p),
                "INLINE lists {p}, which is not a route"
            );
        }
        // The joined sites' routes (spec §7.2) are owner-only live routes:
        // hidden from the LAN by the path rule.
        for p in [
            "/api/live/sites",
            "/api/live/sites/suggest",
            "/api/live/sites/join",
            "/api/live/sites/split",
            "/api/live/sites/answer",
        ] {
            assert!(crate::live::live_route(p), "{p}");
            assert!(routes(src).iter().any(|(r, _)| r == p), "{p} is not routed");
        }
        // `/mcp` is a `route_service` behind the token.
        let mcp = include_str!("mcp.rs");
        assert!(mcp.contains(".route_service(\"/mcp\"") && mcp.contains("require_token"));
        assert_eq!(routes(mcp).len(), 0, "a new MCP route: check it here");
    }
}

/// Every route's class for agents (spec §7.5, §8.6, M7, M8): each handler
/// declares it in its signature, as `Agent<N>` (with `N`'s `need!`),
/// `DeniedToGrants` or `GrantFlow`; the declarations match the spec's route
/// table; no handler reads credentials itself; every Denied handler that
/// builds a view of others' content or streams events refuses the owner's
/// agents; and every plugin runs `clax` through its wrapper.
#[cfg(test)]
mod agents {
    use super::l10::{
        function, handlers, method_handlers, module_source, module_source_opt, routes,
    };
    use crate::agent::Class;
    use clax_core::rights::RightType;

    /// The route table of spec §7.5 for the routes an agent reaches, copied
    /// here as data: method, path, class, right (type and action).
    /// A row of the table: method, path, class, right.
    type Row = (
        &'static str,
        &'static str,
        Class,
        Option<(RightType, &'static str)>,
    );

    const EXPECTED: &[Row] = {
        use Class::*;
        use RightType::*;
        &[
            ("GET", "/api/artifacts", Filtered, Some((Artifacts, "read"))),
            (
                "POST",
                "/api/artifacts",
                Filtered,
                Some((Artifacts, "create")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}",
                Filtered,
                Some((Artifacts, "read")),
            ),
            (
                "PATCH",
                "/api/artifacts/{aid}",
                Filtered,
                Some((Artifacts, "update")),
            ),
            (
                "DELETE",
                "/api/artifacts/{aid}",
                Own,
                Some((Artifacts, "delete")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/versions",
                Filtered,
                Some((Artifacts, "read")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/versions/{n}",
                Filtered,
                Some((Artifacts, "read")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/versions",
                Filtered,
                Some((Artifacts, "update")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/files",
                Filtered,
                Some((Artifacts, "read")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/versions/{n}/files/{*path}",
                Filtered,
                Some((Artifacts, "read")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/assets",
                Filtered,
                Some((Artifacts, "read")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/assets",
                Filtered,
                Some((Artifacts, "update")),
            ),
            (
                "DELETE",
                "/api/artifacts/{aid}/assets/{asset_id}",
                Own,
                Some((Artifacts, "delete")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/presence",
                Own,
                Some((Artifacts, "read")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/working",
                Filtered,
                Some((Comments, "watch")),
            ),
            ("GET", "/api/working", Filtered, Some((Comments, "watch"))),
            ("GET", "/api/threads", Filtered, Some((Comments, "read"))),
            (
                "GET",
                "/api/artifacts/{aid}/threads",
                Filtered,
                Some((Comments, "read")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/threads/{tid}",
                Filtered,
                Some((Comments, "read")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/threads/{tid}/clip",
                Filtered,
                Some((Comments, "read")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/threads",
                Filtered,
                Some((Comments, "write")),
            ),
            // An agent reply needs `reply`; any other comment also `write`.
            (
                "POST",
                "/api/artifacts/{aid}/threads/{tid}/comments",
                Filtered,
                Some((Comments, "reply")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/threads/{tid}/resolve",
                Filtered,
                Some((Comments, "resolve")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/threads/{tid}/reopen",
                Filtered,
                Some((Comments, "resolve")),
            ),
            (
                "DELETE",
                "/api/artifacts/{aid}/threads/{tid}",
                Own,
                Some((Comments, "resolve")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/threads/{tid}/send",
                Filtered,
                Some((Comments, "send")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/threads:send",
                Filtered,
                Some((Comments, "send")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/docs/{*path}",
                Filtered,
                Some((Db, "read")),
            ),
            (
                "PUT",
                "/api/artifacts/{aid}/docs/{*path}",
                Filtered,
                Some((Db, "write")),
            ),
            (
                "PATCH",
                "/api/artifacts/{aid}/docs/{*path}",
                Filtered,
                Some((Db, "write")),
            ),
            (
                "DELETE",
                "/api/artifacts/{aid}/docs/{*path}",
                Filtered,
                Some((Db, "write")),
            ),
            (
                "GET",
                "/api/artifacts/{aid}/docs",
                Filtered,
                Some((Db, "read")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/docs:batch",
                Filtered,
                Some((Db, "write")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/docs:str_replace",
                Filtered,
                Some((Db, "write")),
            ),
            (
                "POST",
                "/api/artifacts/{aid}/docs:acquire",
                Filtered,
                Some((Db, "write")),
            ),
            ("POST", "/api/sessions", Own, None),
            ("POST", "/api/sessions/join", Own, None),
            ("GET", "/api/sessions", Own, None),
            ("GET", "/api/sessions/{id}", Own, None),
            ("PATCH", "/api/sessions/{id}", Own, None),
            (
                "GET",
                "/api/sessions/{id}/watches",
                Own,
                Some((Comments, "watch")),
            ),
            (
                "PUT",
                "/api/sessions/{id}/watches/{aid}",
                Own,
                Some((Comments, "watch")),
            ),
            (
                "DELETE",
                "/api/sessions/{id}/watches/{aid}",
                Own,
                Some((Comments, "watch")),
            ),
            (
                "GET",
                "/api/sessions/{id}/feedback",
                Filtered,
                Some((Comments, "watch")),
            ),
            (
                "GET",
                "/api/sessions/{id}/notices",
                Filtered,
                Some((Comments, "watch")),
            ),
            (
                "POST",
                "/api/sessions/{id}/feedback/ack",
                Own,
                Some((Comments, "watch")),
            ),
            (
                "GET",
                "/api/sessions/{id}/working",
                Own,
                Some((Comments, "watch")),
            ),
            (
                "POST",
                "/api/sessions/{id}/working/renew",
                Own,
                Some((Comments, "watch")),
            ),
            (
                "POST",
                "/api/sessions/{id}/working/end",
                Own,
                Some((Comments, "watch")),
            ),
            (
                "PUT",
                "/api/sessions/{id}/working/{aid}",
                Own,
                Some((Comments, "watch")),
            ),
            (
                "DELETE",
                "/api/sessions/{id}/working/{aid}",
                Own,
                Some((Comments, "watch")),
            ),
            (
                "POST",
                "/api/sessions/{id}/questions",
                Own,
                Some((Ask, "ask")),
            ),
            (
                "GET",
                "/api/sessions/{id}/questions/{qid}",
                Own,
                Some((Ask, "ask")),
            ),
            (
                "POST",
                "/api/sessions/{id}/questions/{qid}/withdraw",
                Own,
                Some((Ask, "ask")),
            ),
            (
                "POST",
                "/api/sessions/{id}/questions/{qid}/release",
                Own,
                Some((Ask, "ask")),
            ),
            (
                "POST",
                "/api/sessions/{id}/questions:terminal",
                Own,
                Some((Ask, "ask")),
            ),
            (
                "POST",
                "/api/sessions/{id}/approvals",
                Own,
                Some((Ask, "ask")),
            ),
            (
                "GET",
                "/api/sessions/{id}/approvals/{rid}",
                Own,
                Some((Ask, "ask")),
            ),
            (
                "POST",
                "/api/sessions/{id}/threads/{tid}/handoff",
                Filtered,
                Some((Comments, "reply")),
            ),
            ("GET", "/api/approvals", Own, None),
            ("GET", "/api/approvals/requests", Own, None),
            ("GET", "/api/agents", Own, None),
            ("POST", "/api/sessions/{id}/tool-calls", Own, None),
            ("POST", "/api/sessions/{id}/tool-call-ids", Own, None),
        ]
    };

    /// The routes that are Filtered for the owner's agents and Denied to a
    /// person's (spec §7.5: `GET /api/threads`).
    const OWNER_AGENTS_ONLY: &[(&str, &str)] = &[("GET", "/api/threads")];

    /// The grant flow's handlers (spec §7.5 `Grant`).
    const GRANT_FLOW: &[&str] = &[
        "grants::request",
        "grants::poll",
        "grants::disconnect",
        "grants::revoke_token",
    ];

    /// What builds a thread, comment, artifact, version, document or
    /// working view in a handler's source (the `Own` rule).
    const CONTENT_VIEWS: &[&str] = &[
        "thread_view(",
        "thread_views(",
        "comment_view(",
        "artifact_view(",
        "with_owner(",
        "version_view(",
        "list_versions(",
        "doc_view(",
        "doc_get(",
        "doc_query(",
        "working_view(",
        "working.for_artifact(",
        "w.for_artifact(",
        "working.roster(",
        "working.all(",
    ];

    /// What else builds a view of others' content (question, inbox item,
    /// person) or streams events (the `not_for_agents` rule).
    const PEOPLE_VIEWS: &[&str] = &[
        "question_view(",
        "questions::view(",
        "inbox_view(",
        "inbox_views(",
        "grant_view(",
        "invite_view(",
        "person_view(",
        "agent_view(",
        "Sse::new(",
        "WebSocketUpgrade",
        "stream_response(",
    ];

    /// Denied handlers the spec names as refusing the owner's agents
    /// whatever their source shows (spec §7.5).
    const MUST_REFUSE: &[&str] = &[
        "people::list_people",
        "people::me",
        "people::set_rights",
        "people::revoke_person",
        "people::narrow_grant",
        "people::narrow_agent",
        "people::elevate_agent",
        "people::unelevate_agent",
        "people::end_agent",
        "inbox::list",
        "inbox::summary",
        "inbox::get_one",
        "inbox::read_one",
        "inbox::unread_one",
        "inbox::read_many",
        "questions::list",
        "questions::get_one",
        "questions::answer",
        "questions::decline",
        "questions::release_owner",
        "live::site",
        "events::events",
        "stream::open",
        "stream::update",
        "grants::list",
        "grants::list_invites",
        "grants::create_invite",
        "grants::revoke_invite",
        "assets::blob",
        "content::index",
        "content::file",
        "shell::artifact_page",
        "people::rename_agent",
        "extension::mint",
    ];

    /// Routes whose handler is not a module function: each, and the text
    /// that declares its class.
    fn inline() -> Vec<(&'static str, &'static str)> {
        let src = include_str!("mod.rs");
        let mcp = include_str!("mcp.rs");
        vec![
            ("/api/admin/shutdown", {
                let at = &src[src.find("\"/api/admin/shutdown\"").unwrap()..];
                &at[..at.find("async move").unwrap()]
            }),
            ("/api/_test/sleep/{ms}", function_any(src, "test_sleep")),
            (
                "/api/_test/slow_publish/{ms}",
                function_any(src, "test_slow_publish"),
            ),
            ("/mcp", function_any(mcp, "require_token")),
        ]
    }

    /// The signature (to the first `{`) of `fn name` in `src`, public or not.
    fn function_any<'a>(src: &'a str, name: &str) -> &'a str {
        let start = src
            .find(&format!("async fn {name}("))
            .unwrap_or_else(|| panic!("no fn {name}"));
        let rest = &src[start..];
        &rest[..rest.find('{').unwrap()]
    }

    /// One routed handler: method, path, `module::function`, signature and
    /// whole source.
    struct Handler {
        method: String,
        path: String,
        name: String,
        sig: &'static str,
        src: &'static str,
    }

    /// Every handler routed in this module.
    fn every_handler() -> Vec<Handler> {
        let src = include_str!("mod.rs");
        let src = &src[..src.find("mod l10 {").unwrap()];
        let mut out = Vec::new();
        for (path, text) in routes(src) {
            for (method, m, f) in method_handlers(&text) {
                let Some(module) = module_source_opt(&m) else {
                    continue;
                };
                let fsrc = function(module, &f);
                out.push(Handler {
                    method,
                    path: path.clone(),
                    name: format!("{m}::{f}"),
                    sig: &fsrc[..fsrc.find('{').unwrap()],
                    src: fsrc,
                });
            }
        }
        out
    }

    /// The need an `Agent<…>` or `TokenOrAgent<…>` in `sig` names.
    fn agent_need(sig: &str) -> Option<&str> {
        let at = sig.find("Agent<")? + "Agent<".len();
        let rest = &sig[at..];
        rest[..rest.find('>')?].rsplit("::").next()
    }

    fn class_count(sig: &str) -> usize {
        sig.matches("Agent<").count()
            + sig.matches("DeniedToGrants").count()
            + sig.matches("GrantFlow").count()
    }

    #[test]
    fn every_route_is_classified_for_agents() {
        let mut used = std::collections::HashSet::new();
        let mut expected_seen = std::collections::HashSet::new();
        for h in every_handler() {
            let (method, path, name) = (&h.method, &h.path, &h.name);
            assert_eq!(
                class_count(h.sig),
                1,
                "{method} {path}: {name} must take exactly one of Agent<N>, DeniedToGrants, GrantFlow"
            );
            let expected = EXPECTED
                .iter()
                .find(|(m, p, _, _)| m == method && p == path);
            if let Some(n) = agent_need(h.sig) {
                let (_, _, class, right) =
                    expected.unwrap_or_else(|| panic!("{method} {path}: {name} takes Agent<{n}>, but spec §7.5 gives agents no such route"));
                expected_seen.insert((method.clone(), path.clone()));
                let info = crate::agent::need_of(&method.parse().unwrap(), path)
                    .unwrap_or_else(|| panic!("{method} {path}: agent::ROUTES lacks it"));
                assert_eq!(info.name, n, "{method} {path}: {name}");
                assert_eq!(
                    (info.class, info.right),
                    (*class, *right),
                    "{method} {path}: {name} vs spec §7.5"
                );
                assert_eq!(
                    !info.grants,
                    OWNER_AGENTS_ONLY.contains(&(method.as_str(), path.as_str())),
                    "{method} {path}: whether a person's agent may use it"
                );
                let entry = crate::agent::ROUTE_NEEDS
                    .iter()
                    .find(|(hn, _, _)| hn == name)
                    .unwrap_or_else(|| panic!("ROUTE_NEEDS lacks {name}"));
                assert_eq!((entry.1, entry.2), (*right, *class), "ROUTE_NEEDS {name}");
                used.insert(name.clone());
                // The feedback routes hand over what a take filtered under
                // the session's `Visibility` (clax-core's delivery guard
                // checks that every feedback item and notice is built
                // under one).
                if info.class == Class::Filtered && crate::agent::FILTERED_LIVE.contains(&info.name)
                {
                    assert!(
                        h.src.contains("vis.filter_")
                            || ["st.take_feedback_readable(", "st.take_notices("]
                                .iter()
                                .any(|t| h.src.contains(t)),
                        "{name}: a Filtered route open to agents must pass its response through vis.filter_"
                    );
                }
                if info.class == Class::Own {
                    for v in CONTENT_VIEWS {
                        assert!(!h.src.contains(v), "{name} is Own but builds a view ({v})");
                    }
                    // A question's view carries its artifact's title, an
                    // author field: an agent gets it through its
                    // Visibility (`for_asker`, `Vis::filter_question`).
                    assert!(
                        !h.src.contains("question_view(")
                            || h.src.contains("for_asker(")
                            || h.src.contains("session_close("),
                        "{name} hands an agent a question view outside its Visibility"
                    );
                }
            } else {
                assert!(
                    expected.is_none(),
                    "{method} {path}: {name} takes no Agent<N>, but spec §7.5 opens it to agents"
                );
                if h.sig.contains("GrantFlow") {
                    assert!(
                        GRANT_FLOW.contains(&name.as_str()),
                        "{name} is no grant-flow route"
                    );
                } else {
                    assert!(
                        !GRANT_FLOW.contains(&name.as_str()),
                        "{name} must take GrantFlow"
                    );
                }
            }
        }
        for (m, p, _, _) in EXPECTED {
            assert!(
                expected_seen.contains(&(m.to_string(), p.to_string())),
                "spec §7.5 opens {m} {p} to agents, but no handler there takes Agent<N>"
            );
        }
        for (h, _, _) in crate::agent::ROUTE_NEEDS {
            assert!(
                used.contains(*h),
                "ROUTE_NEEDS lists {h}, which no route uses"
            );
        }
        for (path, text) in inline() {
            assert!(
                class_count(text) == 1 && text.contains("DeniedToGrants"),
                "{path}: its handler must take DeniedToGrants"
            );
        }
    }

    #[test]
    fn every_denied_view_refuses_owner_agents() {
        let mut checked = 0;
        for h in every_handler() {
            if !h.sig.contains("DeniedToGrants") {
                continue;
            }
            let builds = CONTENT_VIEWS
                .iter()
                .chain(PEOPLE_VIEWS)
                .find(|v| h.src.contains(**v));
            let named = MUST_REFUSE.contains(&h.name.as_str());
            if builds.is_some() || named {
                assert!(
                    h.src.contains(".refuse_agents()?"),
                    "{}: a Denied handler that {} must call refuse_agents()? (403 not_for_agents to the owner's agents)",
                    h.name,
                    builds.map_or("the spec names".to_string(), |v| format!(
                        "builds a view ({v})"
                    ))
                );
                checked += 1;
            }
        }
        assert!(checked >= MUST_REFUSE.len(), "{checked}");
    }

    #[test]
    fn no_handler_reads_credentials_itself() {
        let src = include_str!("mod.rs");
        let src = &src[..src.find("mod l10 {").unwrap()];
        let mut modules: Vec<(String, &str)> = routes(src)
            .iter()
            .flat_map(|(_, t)| handlers(t))
            .map(|(m, _)| m)
            .filter_map(|m| module_source_opt(&m).map(|s| (m, s)))
            .collect();
        modules.push(("mod".into(), src));
        modules.push(("mcp".into(), include_str!("mcp.rs")));
        modules.push(("db_caller".into(), include_str!("../db_caller.rs")));
        modules.dedup_by(|a, b| a.0 == b.0);
        assert!(modules.len() > 20, "{}", modules.len());
        for (m, s) in modules {
            if m == "agent" {
                continue;
            }
            assert!(!s.contains("has_token("), "{m} calls has_token: use Caller");
            assert!(
                !s.contains("AUTHORIZATION"),
                "{m} reads Authorization: use Caller"
            );
            assert!(
                !s.to_lowercase().contains("get(\"authorization\")"),
                "{m} reads Authorization: use Caller"
            );
        }
        let _ = module_source("artifacts");
    }

    /// The text from `start` to the end of its line.
    fn line_at(s: &str, start: usize) -> &str {
        let end = s[start..].find('\n').map_or(s.len(), |e| start + e);
        &s[start..end]
    }

    #[test]
    fn every_plugin_invocation_goes_through_the_wrapper() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let read =
            |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
        // Every copy of the wrapper is the same file.
        let wrapper = read("scripts/ensure-clax.sh");
        for p in ["claude-code", "clax", "clax-grok", "pi"] {
            assert_eq!(
                read(&format!("plugins/{p}/scripts/ensure-clax.sh")),
                wrapper,
                "plugins/{p}/scripts/ensure-clax.sh differs from scripts/ensure-clax.sh"
            );
        }
        // `exec` exports CLAX_CHANNEL=agent but for serve, stop, doctor, init
        // and native-host.
        let exec = &wrapper[wrapper.find("    exec)\n").expect("an exec mode")..];
        let exec = &exec[..exec.find("\n        ;;\n").unwrap()];
        assert!(
            exec.contains("serve|stop|doctor|init|native-host) ;;")
                && exec.contains("*) export CLAX_CHANNEL=agent ;;"),
            "ensure-clax.sh exec must export CLAX_CHANNEL=agent for every mode but serve, stop, doctor, init and native-host"
        );
        let tool_hook = read("scripts/tool-hook.sh");
        assert!(
            tool_hook.contains("ensure-clax.sh\" exec hook"),
            "tool-hook.sh must run ensure-clax.sh exec hook"
        );
        for p in ["claude-code", "clax"] {
            assert_eq!(
                read(&format!("plugins/{p}/scripts/tool-hook.sh")),
                tool_hook,
                "plugins/{p}/scripts/tool-hook.sh"
            );
        }
        let runs_through_wrapper = |cmd: &str| {
            cmd.contains("/scripts/ensure-clax.sh\" exec ")
                || cmd.contains("/scripts/tool-hook.sh\"")
        };
        // Claude Code's slash commands: every `!` line.
        let mut lines = 0;
        for e in std::fs::read_dir(root.join("plugins/claude-code/commands")).unwrap() {
            let path = e.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            for (i, _) in text.match_indices("!`") {
                let cmd = line_at(&text, i);
                lines += 1;
                if cmd.contains("ensure-clax.sh\"`") {
                    continue; // the wrapper alone, printing the binary's path
                }
                assert!(runs_through_wrapper(cmd), "{}: {cmd}", path.display());
            }
        }
        assert!(lines >= 8, "{lines}");
        // Every plugin's hooks.
        for p in ["claude-code", "clax", "clax-grok"] {
            let hooks: serde_json::Value =
                serde_json::from_str(&read(&format!("plugins/{p}/hooks/hooks.json"))).unwrap();
            let mut n = 0;
            for groups in hooks["hooks"].as_object().unwrap().values() {
                for g in groups.as_array().unwrap() {
                    for h in g["hooks"].as_array().unwrap() {
                        let cmd = h["command"].as_str().unwrap();
                        assert!(
                            runs_through_wrapper(cmd),
                            "plugins/{p}/hooks/hooks.json: {cmd}"
                        );
                        n += 1;
                    }
                }
            }
            assert!(n > 0, "{p}");
        }
        // Pi: it resolves the binary through the wrapper (no arguments: the
        // path), then runs it directly only for `--version`, `status --json`
        // and `serve --json`, the last two with CLAX_CHANNEL=agent.
        let daemon = read("plugins/pi/src/daemon.ts");
        let clax = read("plugins/pi/src/clax.ts");
        let client = read("plugins/pi/src/client.ts");
        for (file, text) in [
            ("daemon.ts", &daemon),
            ("clax.ts", &clax),
            ("client.ts", &client),
        ] {
            for (i, _) in text
                .match_indices("execFile(")
                .chain(text.match_indices("spawn("))
            {
                let call = line_at(text, i);
                if call.starts_with("spawn(opener") {
                    continue; // the browser opener, not clax
                }
                let ok = call.contains("[wrapper]")
                    || call.contains("[\"--version\"]")
                    || (call.contains("[\"status\", \"--json\"]")
                        && call.contains("CLAX_CHANNEL: \"agent\""))
                    || (call.contains("execFile(bin, args,")
                        && call.contains("CLAX_CHANNEL: \"agent\"")
                        && daemon.contains("const args = [\"serve\", \"--json\""));
                assert!(ok, "plugins/pi/src/{file}: {call}");
            }
        }
        assert!(
            clax.contains("process.env.CLAX_CHANNEL = \"agent\""),
            "the Pi extension must set CLAX_CHANNEL=agent in Pi's process"
        );
        assert!(
            client.matches("\"x-clax-channel\": \"agent\"").count() >= 2,
            "Pi's DaemonClient must send the marker on every request"
        );
    }
}

//! The router: every route mounted on its handler, each group under its
//! timeout and body limits, and the host and credential checks that wrap
//! them all ([`build_router`]). The debug builds' test routes and the
//! testkit's delay middleware are here too.

use crate::routes::{
    approvals, artifacts, assets, connect, content, docs, drafts, extension, feedback, grants, health,
    inbox, live, people, perf, questions, sample, sessions, sites, snapshot, stream, threads,
    token, toolpath, viewers, watches, working,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::routes::{events, mcp, room};
use crate::shell;
#[cfg(not(target_arch = "wasm32"))]
use axum::error_handling::HandleErrorLayer;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    handler::Handler,
    http::StatusCode,
    routing::{delete, get, post},
};
use clax_http::agent::{self, DeniedToGrants};
use clax_http::auth::RequireToken;
#[cfg(not(target_arch = "wasm32"))]
use clax_http::error::ApiError;
use clax_http::limits;
use clax_http::state::AppState;
use clax_http::{auth, host};
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

/// Every route, unwrapped; `shutdown`, when given, mounts `/api/admin/shutdown`.
fn routes(state: AppState, shutdown: Option<tokio::sync::watch::Sender<bool>>) -> Router {
    // Hash the bridge bundle now, not on the first page served.
    shell::bridge_version(&*state.web);
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any);
    let publish_limit = DefaultBodyLimit::max(clax_http::limits::PUBLISH_BODY_LIMIT);
    let asset_limit = DefaultBodyLimit::max(clax_http::limits::UPLOAD_BODY_LIMIT);
    let ask_limit = DefaultBodyLimit::max(clax_http::limits::ASK_BODY_LIMIT);
    let draft_limit = DefaultBodyLimit::max(drafts::DRAFT_BODY_LIMIT);
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
                    <agent::DocsPut as agent::Need>::LIMIT,
                )))
                .patch(docs::patch)
                .delete(docs::delete),
        )
        .route(
            "/api/artifacts/{aid}/docs:batch",
            post(docs::batch.layer(DefaultBodyLimit::max(clax_http::limits::DOCS_BATCH_LIMIT))),
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
        .route("/api/admin/perf/verify", get(perf::verify_times))
        .route("/api/sessions/{id}/working", get(working::for_session))
        .route("/api/sessions/{id}/working/renew", post(working::renew))
        .route("/api/sessions/{id}/working/end", post(working::end))
        .route(
            "/api/sessions/{id}/working/{aid}",
            axum::routing::put(working::put).delete(working::delete),
        )
        .route(
            "/api/sessions/{id}/drafts/{aid}",
            axum::routing::put(drafts::put.layer(draft_limit)).delete(drafts::delete),
        )
        .route("/api/artifacts/{aid}/drafts", get(drafts::list))
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
        .route("/api/people/me/raises", get(people::raises))
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
    #[cfg(debug_assertions)]
    let api_fast = api_fast
        .route("/api/_test/sleep/{ms}", get(test_sleep))
        .route("/api/_test/slow_publish/{ms}", post(test_slow_publish));
    // The audit context answers every caller, cookie and LAN viewer
    // included (what it reports is the point), so it takes no token and is
    // served by the testkit's daemon alone.
    #[cfg(debug_assertions)]
    let api_fast = if state.test_daemon {
        api_fast.route("/api/_test/audit/ctx", get(test_audit_ctx))
    } else {
        api_fast
    };
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
        .route("/api/_test/agent/gate", get(agent::test_gate))
        .route("/api/_test/caller", get(agent::test_caller))
        .route("/api/_test/sessions/{id}/caller", get(agent::test_caller))
        .route("/api/_test/grants/index", get(grants::test_index))
        .route(
            "/api/_test/grants/index/fail",
            post(grants::test_fail_index),
        );
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    let api_fast = api_fast.route("/api/_test/events/open", get(events::open_streams));
    #[cfg(debug_assertions)]
    let api_fast = if state.test_daemon {
        api_fast.layer(axum::middleware::from_fn(test_delay))
    } else {
        api_fast
    };
    let api_fast = api_fast.layer(axum::middleware::from_fn(clax_http::http_cache::api_etag));
    let api_fast =
        with_timeout(api_fast, state.request_timeout).layer(clax_http::http_cache::compression());
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
            post(threads::create.layer(DefaultBodyLimit::max(limits::THREAD_BODY_LIMIT))),
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
    #[cfg(debug_assertions)]
    let api_slow = if state.test_daemon {
        api_slow.layer(axum::middleware::from_fn(test_delay))
    } else {
        api_slow
    };
    let api_slow =
        with_timeout(api_slow, state.publish_timeout).layer(clax_http::http_cache::compression());
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
        .layer(clax_http::http_cache::compression());
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
            "/c/{aid}/draft/{agent}/{token}",
            get(drafts::redirect_to_slash),
        )
        .route("/c/{aid}/draft/{agent}/{token}/", get(drafts::index))
        .route("/c/{aid}/draft/{agent}/{token}/{*path}", get(drafts::file))
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
        #[cfg(debug_assertions)]
        let mcp = if state.test_daemon {
            mcp.layer(axum::middleware::from_fn(test_delay))
        } else {
            mcp
        };
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
            post(move |_d: DeniedToGrants, _t: RequireToken| {
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
        agent::verify_layer,
    ));
    // Live pages answer 404 to callers that may not see them, before any
    // handler runs (spec 2026-10-05-chrome-overlay-design L10).
    // The extension gateway runs first (the last layer is outermost): only
    // what it admits from the extension's origin reaches the rest (spec
    // 2026-10-05-chrome-overlay-design L5).
    r.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        clax_http::live_access::hide_live_pages,
    ))
    .layer(axum::middleware::from_fn_with_state(
        state.clone(),
        clax_http::extension::gateway,
    ))
    .with_state(state)
}

/// Sleeps for the milliseconds in the `x-clax-test-delay-ms` request
/// header, if any, before handling the request. It sits inside each route
/// group's timeout layer, so tests can make a real route slow. Debug
/// builds add it only when [`AppState::test_daemon`] is set: the testkit's
/// daemon, never one `serve` starts.
#[cfg(debug_assertions)]
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
#[cfg(debug_assertions)]
async fn test_audit_ctx(ctx: clax_core::audit::AuditCtx) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "actor": ctx.actor,
        "via": ctx.via,
        "git": ctx.git.context(),
        "git_capture": ctx.git.capture(),
        "call": ctx.call,
    }))
}

/// The longest sleep the test routes take: they hold a request, or a
/// store worker, for at most this long.
#[cfg(debug_assertions)]
const TEST_SLEEP_MAX_MS: u64 = 60_000;

/// `ms`, refused (400 `bad_request`) above [`TEST_SLEEP_MAX_MS`].
#[cfg(debug_assertions)]
fn test_sleep_ms(ms: u64) -> Result<Duration, ApiError> {
    if ms > TEST_SLEEP_MAX_MS {
        return Err(ApiError::bad_request(
            "bad_request",
            format!("sleeps are at most {TEST_SLEEP_MAX_MS} ms"),
        ));
    }
    Ok(Duration::from_millis(ms))
}

/// Sleeps `ms` milliseconds, then answers 200: the owner's token only.
#[cfg(debug_assertions)]
async fn test_sleep(
    _d: DeniedToGrants,
    _t: RequireToken,
    axum::extract::Path(ms): axum::extract::Path<u64>,
) -> Result<StatusCode, ApiError> {
    tokio::time::sleep(test_sleep_ms(ms)?).await;
    Ok(StatusCode::OK)
}

/// Creates an artifact after sleeping inside the blocking closure, publishing the event from the
/// same closure, so tests can observe write side effects surviving a handler timeout. The
/// owner's token only.
#[cfg(debug_assertions)]
async fn test_slow_publish(
    axum::extract::State(s): axum::extract::State<AppState>,
    _d: DeniedToGrants,
    _t: RequireToken,
    axum::extract::Path(ms): axum::extract::Path<u64>,
) -> Result<StatusCode, ApiError> {
    let sleep = test_sleep_ms(ms)?;
    let events = s.events.clone();
    s.store_call(move |st| {
        std::thread::sleep(sleep);
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

/// The daemon's router.
pub fn build_router(state: AppState) -> axum::Router {
    wrap(state.clone(), routes(state, None))
}

/// The daemon's router with `POST /api/admin/shutdown`, which sends on
/// `shutdown`.
pub fn build_router_with_shutdown(
    state: AppState,
    shutdown: tokio::sync::watch::Sender<bool>,
) -> axum::Router {
    wrap(state.clone(), routes(state, Some(shutdown)))
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

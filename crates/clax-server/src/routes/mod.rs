pub mod artifacts;
pub mod assets;
pub mod content;
pub mod docs;
pub mod events;
pub mod feedback;
pub mod health;
pub mod mcp;
pub mod room;
pub mod sample;
pub mod sessions;
pub mod shell;
pub mod stream;
pub mod threads;
pub mod token;
pub mod viewers;
pub mod watches;
pub mod working;

use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use axum::error_handling::HandleErrorLayer;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    handler::Handler,
    http::StatusCode,
    routing::{delete, get, post},
};
use std::time::Duration;
use tower::ServiceBuilder;
use tower::timeout::TimeoutLayer;
use tower_http::cors::{Any, CorsLayer};

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

fn with_timeout(router: Router<AppState>, d: Duration) -> Router<AppState> {
    router.layer(
        ServiceBuilder::new()
            .layer(HandleErrorLayer::new(move |e| timeout_error(e, d)))
            .layer(TimeoutLayer::new(d)),
    )
}

pub fn router(state: AppState, shutdown: Option<tokio::sync::watch::Sender<bool>>) -> Router {
    // Hash the bridge bundle now, not on the first page served.
    shell::bridge_version();
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any);
    let publish_limit = DefaultBodyLimit::max(artifacts::PUBLISH_BODY_LIMIT);
    let asset_limit = DefaultBodyLimit::max(21 * 1024 * 1024);
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
            "/api/artifacts/{aid}/threads/{tid}/resolve",
            post(threads::resolve),
        )
        .route(
            "/api/artifacts/{aid}/threads/{tid}/reopen",
            post(threads::reopen),
        )
        .route("/api/viewers", get(viewers::lookup))
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
            "/api/sessions/{id}/watches/{aid}",
            axum::routing::put(watches::put).delete(watches::delete),
        )
        .route("/api/sessions/{id}/feedback/ack", post(feedback::ack))
        .route("/api/artifacts/{aid}/docs", get(docs::list))
        .route(
            "/api/artifacts/{aid}/docs/{*path}",
            get(docs::get)
                .put(docs::put)
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
        .route("/api/sessions/{id}/working", get(working::for_session))
        .route("/api/sessions/{id}/working/renew", post(working::renew))
        .route("/api/sessions/{id}/working/end", post(working::end))
        .route(
            "/api/sessions/{id}/working/{aid}",
            axum::routing::put(working::put).delete(working::delete),
        );
    #[cfg(feature = "test-routes")]
    let api_fast = api_fast
        .route("/api/_test/sleep/{ms}", get(test_sleep))
        .route("/api/_test/slow_publish/{ms}", post(test_slow_publish));
    #[cfg(debug_assertions)]
    let api_fast = api_fast
        .route("/api/_test/working/skew", post(working::skew))
        .route("/api/_test/events/open", get(events::open_streams))
        .route("/api/_test/stream/open", get(stream::open_streams));
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
        );
    #[cfg(feature = "test-routes")]
    let api_slow = api_slow.layer(axum::middleware::from_fn(test_delay));
    let api_slow =
        with_timeout(api_slow, state.publish_timeout).layer(crate::http_cache::compression());
    let shell_routes = Router::new()
        .route("/", get(shell::gallery_page))
        .route("/a/{aid}", get(shell::artifact_page))
        .route("/a/{aid}/", get(shell::artifact_page))
        .route("/a/{aid}/v/{n}", get(shell::artifact_page))
        // `/a/<id>[/v/<n>]/<file>`: the shell reads the version and page from the path.
        .route("/a/{aid}/{*rest}", get(shell::artifact_page))
        .route("/_clax/{*path}", get(shell::static_file))
        .layer(crate::http_cache::compression());
    let mcp = mcp::router(&state);
    #[cfg(feature = "test-routes")]
    let mcp = mcp.layer(axum::middleware::from_fn(test_delay));
    let mut r = Router::new()
        .route("/healthz", get(health::healthz).layer(cors))
        .route("/api/events", get(events::events))
        .route("/api/stream", get(stream::open))
        .route("/api/artifacts/{aid}/room", get(room::room))
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
        .merge(api_fast)
        .merge(api_slow)
        .merge(shell_routes)
        .route("/_blob/{asset_id}", get(assets::blob))
        .route("/c/{aid}/v/{n}", get(content::redirect_to_slash))
        .route("/c/{aid}/v/{n}/", get(content::index))
        .route("/c/{aid}/v/{n}/{*path}", get(content::file))
        .route(
            "/api/artifacts/{aid}/versions/{n}/files/{*path}",
            get(content::raw_file),
        )
        .merge(mcp);
    if let Some(tx) = shutdown {
        let tx = std::sync::Arc::new(tx);
        r = r.route(
            "/api/admin/shutdown",
            post(move |_t: RequireToken| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(true);
                    StatusCode::ACCEPTED
                }
            }),
        );
    }
    r.with_state(state)
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

#[cfg(feature = "test-routes")]
async fn test_sleep(axum::extract::Path(ms): axum::extract::Path<u64>) -> StatusCode {
    tokio::time::sleep(Duration::from_millis(ms)).await;
    StatusCode::OK
}

/// Creates an artifact after sleeping inside the blocking closure, publishing the event from the
/// same closure, so tests can observe write side effects surviving a handler timeout.
#[cfg(feature = "test-routes")]
async fn test_slow_publish(
    axum::extract::State(s): axum::extract::State<AppState>,
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
        let (artifact, version) = st.create_artifact(p, None)?;
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

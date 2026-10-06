pub mod artifacts;
pub mod assets;
pub mod content;
pub mod docs;
pub mod events;
pub mod extension;
pub mod feedback;
pub mod health;
pub mod live;
pub mod mcp;
pub mod questions;
pub mod room;
pub mod sample;
pub mod sessions;
pub mod shell;
pub mod sites;
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
        .route("/api/_test/stream/open", get(stream::open_streams))
        .route(
            "/api/_test/questions/{qid}/waiters",
            get(questions::waiters),
        )
        .route(
            "/api/_test/sessions/{id}/feedback/waiters",
            get(feedback::waiters),
        );
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
        // A long-poll: outside the request timeout, as the feedback poll.
        .route("/api/sessions/{id}/questions/{qid}", get(questions::poll))
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
        /// It takes `RequireToken`: the token sees live pages.
        Token,
        /// It takes `SeesLive` and checks the artifact its query or body names.
        SeesLive,
        /// A listing or stream that leaves live pages out unless
        /// `sees_live_pages`.
        Filtered,
        /// It names no artifact.
        NoArtifact,
        /// It refuses every caller but the owner (`owner(&who)`); every
        /// owner credential sees live pages (the extension's is
        /// loopback-only).
        Owner,
    }
    use Guard::*;

    /// The handlers of the routes the path rule does not cover.
    const GUARDS: &[(&str, Guard)] = &[
        ("token::token", NoArtifact),
        ("artifacts::list", Filtered),
        ("artifacts::create", Token),
        ("sessions::list", Token),
        ("sessions::register", Token),
        ("sessions::join", Token),
        ("sessions::get", Token),
        ("sessions::patch", Token),
        ("sessions::push_status", NoArtifact),
        ("sample::daemon", Token),
        ("threads::list_all", Token),
        ("working::roster", Token),
        ("working::for_session", Token),
        ("working::renew", Token),
        ("working::end", Token),
        ("working::put", Token),
        ("working::delete", Token),
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
        ("watches::list", Token),
        ("watches::put", Token),
        ("watches::delete", Token),
        ("watches::live_put", Token),
        ("watches::live_delete", Token),
        ("feedback::ack", Token),
        ("feedback::poll", Token),
        ("feedback::notices", Token),
        ("questions::create", Token),
        ("questions::poll", Token),
        ("questions::withdraw", Token),
        ("questions::release", Token),
        ("questions::terminal", Token),
        ("questions::waiters", Token),
        ("feedback::waiters", Token),
        ("questions::list", Owner),
        ("questions::get_one", Owner),
        ("questions::answer", Owner),
        ("questions::decline", Owner),
        ("questions::release_owner", Owner),
        ("shell::gallery_page", NoArtifact),
        ("shell::static_file", NoArtifact),
        ("health::healthz", NoArtifact),
        ("assets::blob", SeesLive),
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
        ("/api/admin/shutdown", "RequireToken"),
    ];

    fn module_source(m: &str) -> &'static str {
        match m {
            "artifacts" => include_str!("artifacts.rs"),
            "assets" => include_str!("assets.rs"),
            "content" => include_str!("content.rs"),
            "docs" => include_str!("docs.rs"),
            "events" => include_str!("events.rs"),
            "extension" => include_str!("extension.rs"),
            "feedback" => include_str!("feedback.rs"),
            "health" => include_str!("health.rs"),
            "live" => include_str!("live.rs"),
            "questions" => include_str!("questions.rs"),
            "room" => include_str!("room.rs"),
            "sample" => include_str!("sample.rs"),
            "sessions" => include_str!("sessions.rs"),
            "shell" => include_str!("shell.rs"),
            "stream" => include_str!("stream.rs"),
            "threads" => include_str!("threads.rs"),
            "token" => include_str!("token.rs"),
            "viewers" => include_str!("viewers.rs"),
            "watches" => include_str!("watches.rs"),
            "working" => include_str!("working.rs"),
            _ => panic!("no route module {m}"),
        }
    }

    /// Each `.route("<path>", <handlers>)` of `src`: the path and the
    /// handlers' text.
    fn routes(src: &str) -> Vec<(String, String)> {
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
    fn handlers(text: &str) -> Vec<(String, String)> {
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
            if !m.is_empty() && !f.is_empty() && m != "axum" && m != "routing" {
                out.push((m.to_string(), f.to_string()));
            }
            i = at + 2;
        }
        out
    }

    /// The text of `pub async fn name` in `src`: its signature and body.
    fn function<'a>(src: &'a str, name: &str) -> &'a str {
        let start = src
            .find(&format!("pub async fn {name}("))
            .unwrap_or_else(|| panic!("no handler {name}"));
        let rest = &src[start..];
        let end = rest.find("\n}\n").map_or(rest.len(), |e| e + 3);
        &rest[..end]
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
                    SeesLive => assert!(
                        sig.contains("SeesLive") && src.contains(".check("),
                        "{name} does not check SeesLive"
                    ),
                    Filtered => assert!(
                        src.contains("sees_live_pages"),
                        "{name} does not filter by sees_live_pages"
                    ),
                    Owner => assert!(
                        sig.contains("who: Identity") && src.contains("owner(&who)?;"),
                        "{name} does not refuse callers other than the owner"
                    ),
                    NoArtifact => assert!(
                        !src.contains("ArtifactId") && !src.contains("parse_id"),
                        "{name} names an artifact"
                    ),
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

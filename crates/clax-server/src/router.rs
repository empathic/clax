//! The router: every route mounted on its handler, each group under its
//! timeout and body limits, and the host and credential checks that wrap
//! them all ([`build_router`]). The debug builds' test routes and the
//! testkit's delay middleware are here too.

use crate::routes::{
    admin, app_bundles, app_drafts, approvals, artifacts, assets, audit_trail, bot, client_imports,
    confirmations, connect, content, discovery, docs, drafts, extension, feedback, git_token,
    grants, health, inbox, jobs, live, live_layers, login, mcp_http, people, perf, permissions,
    questions, queue_agent, queue_reporter, queues, remote_apps, remotes, resolve, room, sample,
    sessions, settings, site_signin, sites, snapshot, stream, surface, tags, threads, token,
    toolpath, viewers, watches, working,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::routes::{events, mcp, net};
use crate::shell;
use crate::shell::get_page;
#[cfg(not(target_arch = "wasm32"))]
use axum::error_handling::HandleErrorLayer;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    handler::Handler,
    http::StatusCode,
    routing::{delete, get, patch, post, put},
};
use clax_http::agent::{self, DeniedToGrants};
use clax_http::auth::RequireToken;
#[cfg(any(debug_assertions, not(target_arch = "wasm32")))]
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
        .route("/api/browser-login", post(login::create))
        .route("/api/confirmations", get(confirmations::list))
        .route("/api/artifacts", get(artifacts::list))
        .route(
            "/api/sessions",
            get(sessions::list).post(sessions::register),
        )
        .route("/api/sessions/join", post(sessions::join))
        .route("/api/push", get(sessions::push_status))
        .route("/api/sample", get(sample::daemon))
        .route("/api/sample/key", axum::routing::put(sample::put_key))
        .route("/api/core/trim", post(health::trim))
        .route("/api/jobs/owner", post(jobs::claim).delete(jobs::release))
        .route("/api/jobs", get(jobs::list).post(jobs::create))
        .route("/api/jobs/{id}", get(jobs::get_one).patch(jobs::patch))
        .route(
            "/api/jobs/{id}/threads/{tid}",
            axum::routing::put(jobs::put_thread).delete(jobs::delete_thread),
        )
        .route(
            "/api/jobs/{id}/questions/{qid}",
            axum::routing::put(jobs::put_question),
        )
        .route("/api/agent-read-consents", get(jobs::consents))
        .route(
            "/api/agent-read-consents/{reader}/{source}",
            axum::routing::put(jobs::put_consent).delete(jobs::delete_consent),
        )
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
        .route(
            "/api/artifacts/{aid}/site-code",
            post(site_signin::site_code),
        )
        .route("/api/artifacts/{aid}/head", get(app_bundles::head))
        .route(
            "/api/artifacts/{aid}/versions/{n}/bundle",
            get(app_bundles::get_bundle),
        )
        .route(
            "/api/artifacts/{aid}/installs",
            post(app_bundles::record_install),
        )
        .route("/api/tags", get(tags::list).post(tags::create))
        .route(
            "/api/tags/{tag_id}",
            patch(tags::update).delete(tags::delete),
        )
        .route(
            "/api/artifacts/{aid}/tags",
            put(tags::put_artifact).patch(tags::patch_artifact),
        )
        // Report queues: their admins' routes, then their triage routes
        // (spec 2026-10-27-clax-sdk-design §7). The reporter routes carry
        // their own credentials and are mounted in front of the credential
        // layers ([`wrap`]).
        .route("/api/queues", get(queues::list).post(queues::create))
        .route(
            "/api/queues/{qid}",
            patch(queues::patch).delete(queues::delete),
        )
        .route("/api/queues/{qid}/members", get(queues::members))
        .route(
            "/api/queues/{qid}/members/{public_id}",
            put(queues::put_member).delete(queues::delete_member),
        )
        .route(
            "/api/queues/{qid}/service-grants",
            get(queues::grants).post(queues::create_grant),
        )
        .route(
            "/api/queues/{qid}/service-grants/{gid}",
            delete(queues::revoke_grant),
        )
        .route("/api/queues/{qid}/reports", get(queue_agent::list))
        .route(
            "/api/queues/{qid}/reports/{tid}",
            get(queue_agent::get).patch(queue_agent::patch),
        )
        .route(
            "/api/queues/{qid}/reports/{tid}/claim",
            post(queue_agent::claim),
        )
        .route(
            "/api/queues/{qid}/reports/{tid}/renew",
            post(queue_agent::renew),
        )
        .route(
            "/api/queues/{qid}/reports/{tid}/release",
            post(queue_agent::release),
        )
        .route(
            "/api/queues/{qid}/reports/{tid}/comments",
            post(queue_agent::reply),
        )
        .route(
            "/api/queues/{qid}/reports/{tid}/resolve",
            post(queue_agent::resolve),
        )
        .route(
            "/api/queues/{qid}/reports/{tid}/artifact",
            patch(queue_agent::patch_artifact),
        )
        .route(
            "/api/queues/{qid}/reports/{tid}/clip",
            get(queue_agent::clip),
        )
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
        .route("/api/resolve", get(resolve::get))
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
            "/api/sessions/{id}/where-to-ask",
            get(surface::where_to_ask),
        )
        .route("/api/sessions/{id}/terminal", post(surface::terminal))
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
        .route("/api/questions/{qid}/pull", post(questions::pull))
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
        .route(
            "/api/artifacts/{aid}/docs:collections",
            get(docs::collections),
        )
        .route("/api/drafts", get(app_drafts::list))
        .route("/api/drafts/{id}", delete(app_drafts::delete))
        .route("/api/drafts/{id}/renders", post(app_drafts::render))
        .route("/api/artifacts/{aid}/db-grants", get(docs::grants))
        .route(
            "/api/artifacts/{aid}/db-grants/{source}",
            axum::routing::put(docs::grant).delete(docs::revoke),
        )
        .route(
            "/api/artifacts/{aid}/sample-grant",
            get(sample::grant_status)
                .put(sample::grant)
                .delete(sample::revoke),
        )
        .route(
            "/api/artifacts/{aid}/permissions",
            get(permissions::permissions),
        )
        .route(
            "/api/artifacts/{aid}/capability-approval",
            put(permissions::approve),
        )
        .route(
            "/api/artifacts/{aid}/capability-approval/{name}",
            delete(permissions::withdraw),
        )
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
        .route(
            "/api/artifacts/{aid}/git-token",
            get(git_token::options).post(git_token::mint),
        )
        .route(
            "/api/sessions/{id}/git-token/{aid}",
            post(git_token::session_mint),
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
        .route("/api/people/me/raises", get(people::raises))
        .route(
            "/api/people/me/grants/{gid}/narrow",
            axum::routing::put(people::narrow_grant),
        )
        .route("/api/people/{pid}", delete(people::revoke_person))
        .route(
            "/api/people/{pid}/reinstate",
            post(people::reinstate_person),
        )
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
    // The `net` capability fetches from this machine (sockets, DNS, a disk
    // cache): the daemon's alone, as a Durable Object has none of them.
    #[cfg(not(target_arch = "wasm32"))]
    let api_fast = api_fast.route(
        "/api/artifacts/{aid}/net-grants",
        get(net::grants).put(net::grant).delete(net::revoke),
    );
    #[cfg(debug_assertions)]
    let api_fast = api_fast.route("/api/_test/sleep/{ms}", get(test_sleep));
    // It sleeps a store call's thread, which a Worker has none of.
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    let api_fast = api_fast.route("/api/_test/slow_publish/{ms}", post(test_slow_publish));
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
        .route("/api/_test/viewers", post(login::test_viewer))
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
        .route("/api/artifacts/{aid}/fork", post(artifacts::fork))
        .route("/api/artifacts:new", post(artifacts::create_from_brief))
        // Remotes and pushes wait on a public Clax.
        .route("/api/settings", get(settings::get).put(settings::put))
        // ClaxBot (spec 2026-10-21-claxbot-design §10).
        .route("/api/me/bot", get(bot::me_get).put(bot::me_put))
        .route("/api/me/bot/key", put(bot::key_put).delete(bot::key_delete))
        .route("/api/me/usage", get(bot::usage))
        .route("/api/bot/runs", get(bot::list).post(bot::create))
        .route("/api/bot/runs/{id}", get(bot::get))
        .route("/api/bot/runs/{id}/cancel", post(bot::cancel))
        .route("/api/bot/runs/{id}/draft/", get(bot::draft_index))
        .route("/api/bot/runs/{id}/draft/{*path}", get(bot::draft_file))
        .route("/api/admin/people", get(admin::people))
        .route("/api/admin/people/{person}/flags", put(admin::set_flags))
        .route("/api/admin/people/{person}/role", put(admin::set_role))
        .route(
            "/api/admin/people/{person}/credits",
            put(admin::set_credits),
        )
        .route(
            "/api/admin/provider",
            get(admin::provider).put(admin::set_provider),
        )
        .route(
            "/api/admin/provider/key",
            put(admin::set_key).delete(admin::clear_key),
        )
        .route("/api/admin/provider/test", post(admin::test))
        .route("/api/admin/provider/models", get(admin::models))
        .route(
            "/api/admin/pricing",
            get(admin::pricing).put(admin::set_pricing),
        )
        .route("/api/admin/usage", get(admin::usage))
        .route("/api/audit", get(audit_trail::list))
        .route("/api/audit/{seq}", get(audit_trail::get))
        .route("/api/remotes", get(remotes::list).post(remotes::add))
        .route("/api/remotes/{id}", delete(remotes::remove))
        .route("/api/remotes/{id}/poll", post(remotes::poll))
        .route("/api/artifacts/{aid}/remotes", get(remotes::for_artifact))
        .route(
            "/api/artifacts/{aid}/remotes/{rid}",
            axum::routing::patch(remotes::patch_link).delete(remotes::unlink),
        )
        .route(
            "/api/artifacts/{aid}/remotes/{rid}/retry",
            post(remotes::retry),
        )
        .route(
            "/api/artifacts/{aid}/unsent:drop",
            post(remotes::drop_unsent),
        )
        .route("/api/artifacts/{aid}/push", post(remotes::push))
        // The Clax app's routes to its apps' public copies, through this
        // core's token there (`remote_apps`); a bundle up to 32 MiB.
        .route(
            "/api/artifacts/{aid}/remotes/{rid}/bundle",
            put(remote_apps::put_bundle.layer(DefaultBodyLimit::max(limits::BUNDLE_BODY_LIMIT))),
        )
        .route("/api/remotes/{id}/links", post(remote_apps::link))
        .route("/api/remotes/{id}/apps", get(remote_apps::apps))
        .route(
            "/api/remotes/{id}/artifacts/{raid}",
            delete(remote_apps::unpublish),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/head",
            get(remote_apps::head),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/comments",
            post(remote_apps::comment),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/installs",
            post(remote_apps::record_install),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/versions/{n}/bundle",
            get(remote_apps::bundle),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/versions/{n}/files",
            get(remote_apps::version_files),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/versions/{n}/files/{*path}",
            get(remote_apps::version_file),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/sharing",
            get(remote_apps::get_sharing),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/sharing/link",
            put(remote_apps::put_link),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/sharing/shares",
            put(remote_apps::put_share),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/sharing/shares/{sid}",
            delete(remote_apps::delete_share),
        )
        .route(
            "/api/remotes/{id}/artifacts/{raid}/sharing/comments",
            put(remote_apps::put_comments),
        )
        // A version's bundle, up to 32 MiB.
        .route(
            "/api/artifacts/{aid}/versions/{n}/bundle",
            put(app_bundles::put_bundle.layer(DefaultBodyLimit::max(limits::BUNDLE_BODY_LIMIT))),
        )
        .route("/api/drafts", post(app_drafts::create.layer(publish_limit)))
        // A publish step carries a publish.
        .route(
            "/api/jobs/{id}/steps/{key}",
            post(jobs::commit.layer(publish_limit)),
        )
        // The app's state files, up to its 8 MiB of unfinished changes.
        .route(
            "/api/client-imports/{key}",
            post(client_imports::import.layer(publish_limit)),
        )
        .route(
            "/api/artifacts/{aid}/versions:restore",
            post(artifacts::restore),
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
        .route("/queues", get(shell::gallery_page))
        .route("/tags", get(shell::gallery_page))
        .route("/admin", get(shell::gallery_page))
        .route("/settings", get(shell::gallery_page))
        .route("/me", get(shell::gallery_page))
        .route("/audit", get(shell::gallery_page))
        .route("/a/{aid}", get(shell::artifact_page))
        .route("/a/{aid}/", get(shell::artifact_page))
        .route("/a/{aid}/v/{n}", get(shell::artifact_page))
        // The Get in Clax page, before the file paths: `get` is reserved.
        .route("/a/{aid}/get", get(get_page::page))
        .route("/a/{aid}/get/", get(get_page::page))
        // The Clax app's links, which need no session
        // (`clax_http::hosted::OWN_PAGES`); each checks the host itself.
        .route(
            "/.well-known/apple-app-site-association",
            get(get_page::apple_app_site_association),
        )
        .route("/.well-known/assetlinks.json", get(get_page::assetlinks))
        // What this Clax is, for any caller (`clax_http::hosted::OWN_PAGES`).
        .route("/.well-known/clax", get(discovery::get))
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
        // A long-poll while it is open, and the owner's browser's answer.
        .route(
            "/api/confirmations/{id}",
            get(confirmations::get_one).post(confirmations::answer),
        )
        .merge(api_fast)
        .merge(api_slow)
        .merge(shell_routes)
        // An asset's addresses of before site hosts redirect to its site.
        .route("/_blob/{asset_id}", get(assets::blob))
        .route("/_clax/blob/{asset_id}", get(assets::blob))
        .route("/connect/b/{code}", get(connect::browser_link))
        .route("/login", get(login::redeem))
        .route("/_blob/{aid}/{asset_id}", get(assets::scoped_blob))
        .route("/site-signin", get(site_signin::page))
        .route(
            "/api/artifacts/{aid}/versions/{n}/files/{*path}",
            get(content::raw_file),
        );
    // Bounded by the fetch's own deadline (`net::TIMEOUT`); the daemon's
    // alone, as the grants above.
    #[cfg(not(target_arch = "wasm32"))]
    let r = r.route(
        "/api/artifacts/{aid}/net:fetch",
        post(net::fetch.layer(DefaultBodyLimit::max(net::NET_BODY_LIMIT))),
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
            .route("/api/artifacts/{aid}/live", get(live_layers::live))
            .merge(mcp)
    };
    // The stream's, the room's and the live layers' WebSockets, which the
    // Durable Object upgrades and holds.
    #[cfg(target_arch = "wasm32")]
    let r = r
        .route("/api/stream", get(stream::open_socket))
        .route("/api/artifacts/{aid}/room", get(room::room_socket))
        .route("/api/artifacts/{aid}/live", get(live_layers::live_socket));
    // The tool set's JSON-RPC adapter: `/mcp` on a Durable Object, beside
    // rmcp's `/mcp` in the daemon.
    let r = r.merge(mcp_http::router(&state));
    // A ClaxBot run's events, for as long as the run and its reader last.
    let r = r.route("/api/bot/runs/{id}/events", get(bot::events));
    // A hosted server sends ClaxBot's instance key only to a public https
    // host (a hosted mode set after the state was assembled, as the
    // testkit's, is caught here; a test's own looser policy is kept).
    if state.hosted.is_some()
        && state.store.base_url_policy() == clax_store::bot_settings::BaseUrlPolicy::Local
    {
        state
            .store
            .set_base_url_policy(clax_store::bot_settings::BaseUrlPolicy::Hosted);
    }
    // A hosted daemon's sign-in, sharing and agent-token routes, under the
    // layers below as every route is.
    let mut r = if state.hosted.is_some() {
        r.merge(crate::hosted::router())
    } else {
        r
    };
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
    // A request reading for another app only reads that app's documents: the header is
    // refused on every other route and method.
    let r = r.layer(axum::middleware::from_fn(
        clax_http::db_caller::refuse_reader_elsewhere,
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
        clax_rt::time::sleep(Duration::from_millis(ms)).await;
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

/// The routes whose response lasts as long as its reader does: the event
/// streams, a page's room and live layers (the daemon's block that holds
/// connections open, in [`build_router_with_shutdown`]) and rmcp's `/mcp`.
/// A caller that reads a response whole (the in-process core's `call`)
/// refuses them by [`held_open::matches`]. A test reads the router's
/// source, so a route added to that block fails it until it is listed here.
pub mod held_open {
    /// The owner's change events (SSE).
    pub const EVENTS: &str = "/api/events";
    /// The stream hub's connection (SSE; `POST /api/stream/{id}` is not one).
    pub const STREAM: &str = "/api/stream";
    /// A page's room.
    pub const ROOM: &str = "/api/artifacts/{aid}/room";
    /// A page's live layers.
    pub const LIVE: &str = "/api/artifacts/{aid}/live";
    /// rmcp's streamable HTTP endpoint.
    pub const MCP: &str = "/mcp";
    /// Every one of them.
    pub const ALL: [&str; 5] = [EVENTS, STREAM, ROOM, LIVE, MCP];

    /// Whether `path` (without its query) is one of [`ALL`], a `{…}`
    /// segment standing for any one segment.
    pub fn matches(path: &str) -> bool {
        ALL.iter().any(|route| {
            let mut want = route.split('/');
            let mut got = path.split('/');
            loop {
                match (want.next(), got.next()) {
                    (None, None) => return true,
                    (Some(w), Some(g)) if w == g || (w.starts_with('{') && !g.is_empty()) => {}
                    _ => return false,
                }
            }
        })
    }
}

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
    clax_rt::time::sleep(test_sleep_ms(ms)?).await;
    Ok(StatusCode::OK)
}

/// `POST /api/_test/slow_publish/{ms}`'s query.
#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
#[derive(serde::Deserialize)]
struct SlowPublishQuery {
    /// Wait for the handler to be dropped, `ms` at most, in place of
    /// sleeping `ms`.
    #[serde(default)]
    after_drop: bool,
}

/// Creates an artifact from a blocking closure that first sleeps `ms`
/// milliseconds, publishing the event from the same closure, so tests can
/// observe write side effects surviving a handler timeout. With
/// `?after_drop=true` the closure waits instead until the handler is
/// dropped (its route group's timeout fired, or the client went away), `ms`
/// at most, so the artifact is made after the request has ended however
/// slow the machine. The owner's token only. Native builds only: the wait
/// holds a store worker's thread.
#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
async fn test_slow_publish(
    axum::extract::State(s): axum::extract::State<AppState>,
    _d: DeniedToGrants,
    _t: RequireToken,
    axum::extract::Path(ms): axum::extract::Path<u64>,
    axum::extract::Query(q): axum::extract::Query<SlowPublishQuery>,
) -> Result<StatusCode, ApiError> {
    let sleep = test_sleep_ms(ms)?;
    let events = s.events.clone();
    // Lives in this handler's future: dropping the future disconnects
    // `dropped`.
    let (_alive, dropped) = std::sync::mpsc::channel::<()>();
    s.store_call(move |st| {
        // Tests of the binary wait for this line in its log.
        tracing::info!("slow publish started");
        if q.after_drop {
            // Disconnected once the handler is dropped; times out after `sleep`.
            let _ = dropped.recv_timeout(sleep);
        } else {
            std::thread::sleep(sleep);
        }
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

/// `instance` (the public host's routes, [`routes`]) beside the site routes
/// ([`crate::routes::site::site_routes`]): a request on a site host (one
/// [`host::site_layer`] gave a [`clax_http::host::SiteRequest`]) reaches
/// the site routes alone, any other the instance's alone.
fn by_host(state: &AppState, instance: axum::Router) -> axum::Router {
    // The extension gateway covers the site routes as it does the
    // instance's ([`routes`]): what the extension's origin may reach is its
    // rule's routes alone.
    let site = crate::routes::site::site_routes(state)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            clax_http::extension::gateway,
        ))
        .with_state(state.clone());
    axum::Router::new().fallback(move |req: axum::extract::Request| {
        let to = if req
            .extensions()
            .get::<clax_http::host::SiteRequest>()
            .is_some()
        {
            site.clone()
        } else {
            instance.clone()
        };
        async move {
            use tower::ServiceExt as _;
            let Ok(res) = to.oneshot(req).await;
            res
        }
    })
}

/// The daemon's router with `POST /api/admin/shutdown`, which sends on
/// `shutdown`.
pub fn build_router_with_shutdown(
    state: AppState,
    shutdown: tokio::sync::watch::Sender<bool>,
) -> axum::Router {
    wrap(state.clone(), routes(state, Some(shutdown)))
}

// `Router::layer` runs after route matching, so the site host lookup
// (`host::site_layer`) has to wrap the whole inner router (mounted as the
// outer fallback, [`by_host`]) to choose between the site routes and the
// instance's. The `/api` host check runs after
// the lookup (the later `layer` is outermost),
// then the credential gate, and the header checks of agent credentials
// after all three, before any route.
// On a hosted daemon ([`clax_http::hosted`]), whose own routes `routes`
// mounts, its gate stands in for the `/api` host check, which admits only
// this machine's names.
fn wrap(state: AppState, inner: axum::Router) -> axum::Router {
    let hosted = state.hosted.is_some();
    // Innermost: a request's git token revocations are done before it
    // answers (`git_remotes::settle_layer`).
    let r = by_host(&state, inner)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            clax_http::git_remotes::settle_layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent::authenticate,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_credential,
        ))
        // A local daemon serves a request with no credential only what
        // anyone may reach (`local_trust::gate`).
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            clax_http::local_trust::gate,
        ));
    let r = if hosted {
        r.layer(axum::middleware::from_fn_with_state(
            state.clone(),
            clax_http::hosted::gate,
        ))
        // Outside the gate: its refusals of a Get in Clax page, as a page.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            get_page::refusals,
        ))
    } else {
        r.layer(axum::middleware::from_fn(auth::require_api_host))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                clax_http::identity::local_peers,
            ))
            // Outermost: every layer and route after it sees whether the
            // request carries an owner browser session.
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                clax_http::local_trust::sessions,
            ))
    };
    // A queue's reporter and service routes carry their own credentials
    // and answer other origins: on the instance's host they are matched in
    // front of the layers above, and every other request goes through them.
    // A queue credential stands for no one anywhere else: on any other
    // route, or on a site's host, it is 401 `invalid_token`.
    let elsewhere = r.clone();
    let reporter =
        queue_reporter::router(state.clone()).fallback(move |req: axum::extract::Request| {
            let to = elsewhere.clone();
            async move {
                if clax_http::reporter::carries_queue_credential(req.headers()) {
                    return axum::response::IntoResponse::into_response(
                        clax_http::reporter::refused_elsewhere(),
                    );
                }
                use tower::ServiceExt as _;
                let Ok(res) = to.oneshot(req).await;
                res
            }
        });
    let r = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let site = req
            .extensions()
            .get::<clax_http::host::SiteRequest>()
            .is_some();
        let refused = site && clax_http::reporter::carries_queue_credential(req.headers());
        let to = if site { r.clone() } else { reporter.clone() };
        async move {
            if refused {
                return axum::response::IntoResponse::into_response(
                    clax_http::reporter::refused_elsewhere(),
                );
            }
            use tower::ServiceExt as _;
            let Ok(res) = to.oneshot(req).await;
            res
        }
    });
    r.layer(axum::middleware::from_fn_with_state(
        state,
        host::site_layer,
    ))
}

#[cfg(test)]
mod tests {
    /// A Worker builds this router too, debug test routes included, and has
    /// no tokio timer: the routes sleep on `clax_rt::time` (crate docs).
    #[test]
    fn the_routes_sleep_on_clax_rt() {
        let src = include_str!("router.rs");
        let src = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(!src.contains(concat!("tokio", "::time::")));
    }
}

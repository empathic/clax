//! A queue's routes for a deployer's app (spec 2026-10-27-clax-sdk-design
//! §5.3, §7.5; spec 2026-10-29-feedback-loop-design §5, §6), on their own
//! router ([`router`]), which the daemon mounts in front of its own
//! credential layers: they carry their own credentials
//! ([`clax_http::reporter`]) and answer cross-origin requests.
//!
//! - The app's backend (`Authorization: Clax-Queue <token>` and a
//!   `Clax-Signature` by its service grant's key, no `Origin`): `POST
//!   /api/queues/<qid>/reporter-tokens {reporter_id, origin, public_key,
//!   profile?}` → `{token, expires_at}`.
//! - A reporter in the app's page (`Authorization: Clax-Reporter <token>`,
//!   a `Clax-Signature` by the reporter's key, and the token's `Origin`):
//!   `POST /api/queues/<qid>/reporter/reports` (multipart) → `201
//!   {report}`; `GET /api/queues/<qid>/reporter/reports?cursor=` →
//!   `{reports, next}`; `POST …/reporter/reports/<tid>/comments {body}` →
//!   `{report}`; `GET …/reporter/reports/<tid>/clip` and
//!   `…/comments/<cid>/clip` → the PNG; `POST …/reporter/reports/<tid>/verify
//!   {note?, context?}` and `…/reopen` (multipart) → `{report}`; `GET
//!   …/reporter/reports/<tid>/diagnostics?capture=` → `{lines}`; `POST
//!   …/reporter/reports/<tid>/seen` → 204; `GET
//!   /api/queues/<qid>/reporter/badge` → `{waiting, report_ids}`; `OPTIONS`
//!   on each, the CORS preflight ([`clax_http::reporter::cors`]).
//!
//! The reporter is always the token's, never the body's. A reporter reads
//! the reports its queue's `reporters_see` shows it, and gives a verdict on
//! the ones it filed; any other is 404 `not_found`, as one that does not
//! exist.

use super::assets::multipart_error;
use super::queue_agent::{
    Announcer, CaptureQuery, announce_comment, announce_resolved, last_comment, png,
};
use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use clax_core::audit::{Actor, AuditCtx, Via};
use clax_core::contract::{Body, Channel, Method, Op, Part, Rights, schema};
use clax_core::gitctx::GitField;
use clax_core::queue::{
    Badge, CommentRequest, DiagnosticsAnswer, PageQuery, ReportAnswer, ReportPage, ReporterToken,
    ReporterTokenRequest, VerifyRequest,
};
use clax_core::report::{
    DiagLine, ReportContext, ReporterProfile, ServerStamp, parse_context, parse_diagnostics,
};
use clax_core::{Anchor, CoreError};
use clax_http::error::ApiError;
use clax_http::reporter::{self, REPORT_BODY_LIMIT, ReporterCaller, ServiceCaller};
use clax_http::state::AppState;
use clax_store::queues::{ExternalScope, NewReport, Verdict, Visibility};
use clax_store::threads::clip_problem;
use serde_json::Value;
use std::collections::BTreeMap;

/// The routes' operations in the API contract (`clax_core::contract`), in
/// the router's order: every route here is public.
pub const CONTRACT: &[&Op] = &[
    &MINT_TOKEN,
    &MY_REPORTS,
    &FILE_REPORT,
    &COMMENT,
    &CLIP,
    &COMMENT_CLIP,
    &VERIFY,
    &REOPEN,
    &DIAGNOSTICS,
    &SEEN,
    &BADGE,
];

const REPORTER: &[Channel] = &[Channel::Reporter];

/// A report the token's reporter does not see, or none: 404 `not_found`.
const UNSEEN: (u16, &str) = (404, "not_found");

/// The service and reporter routes, with `state`.
pub fn router(state: AppState) -> Router {
    let reporter = Router::new()
        .route(
            "/api/queues/{qid}/reporter/reports",
            get(my_reports)
                .post(file_report)
                // The extractor bounds the body; `Multipart` reads it again.
                .layer(DefaultBodyLimit::disable())
                .options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/comments",
            post(comment).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/clip",
            get(clip).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/comments/{cid}/clip",
            get(comment_clip).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/verify",
            post(verify).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/reopen",
            post(reopen)
                .layer(DefaultBodyLimit::disable())
                .options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/diagnostics",
            get(diagnostics).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/reports/{tid}/seen",
            post(seen).options(preflight),
        )
        .route(
            "/api/queues/{qid}/reporter/badge",
            get(badge).options(preflight),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            reporter::cors,
        ));
    Router::new()
        .route("/api/queues/{qid}/reporter-tokens", post(mint_token))
        .merge(reporter)
        .with_state(state)
}

/// Who a reporter's change is recorded as: reporter `reporter_id` of queue
/// `queue_id`, no person of this instance, from the network.
fn reporter_ctx(queue_id: &str, reporter_id: &str) -> AuditCtx {
    AuditCtx {
        actor: Actor::Reporter {
            queue_id: queue_id.to_string(),
            reporter_id: reporter_id.to_string(),
        },
        via: Via::Lan,
        git: GitField::Absent,
        call: None,
    }
}

fn json_body<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes).map_err(|e| ApiError::bad_request("invalid_args", e.to_string()))
}

/// The profile `v` names: an object of `name`, `email`, `github` and
/// `avatar_url`, each a string or null; `null` is none.
///
/// # Errors
/// [`CoreError::InvalidProfile`] for anything else.
fn profile_of(v: Option<Value>) -> Result<Option<ReporterProfile>, CoreError> {
    let bad = |m: String| CoreError::InvalidProfile(m);
    let m = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(m)) => m,
        Some(_) => {
            return Err(bad(
                "a profile is an object of name, email, github and avatar_url".into(),
            ));
        }
    };
    for (k, val) in &m {
        if !matches!(k.as_str(), "name" | "email" | "github" | "avatar_url") {
            return Err(bad(format!(
                "a profile has no field '{k}': it holds name, email, github and avatar_url"
            )));
        }
        if !(val.is_string() || val.is_null()) {
            return Err(bad(format!("a profile's {k} is a string")));
        }
    }
    serde_json::from_value(Value::Object(m))
        .map(Some)
        .map_err(|e| bad(e.to_string()))
}

/// `POST /api/queues/<qid>/reporter-tokens {reporter_id, origin,
/// public_key, profile?}`: a reporter token of the caller's service grant
/// for `reporter_id`, bound to `origin` (one of the queue's) and the
/// Ed25519 `public_key` (unpadded base64url). `profile` (`{name?, email?,
/// github?, avatar_url?}`, each at most 256 bytes, `avatar_url` an http(s)
/// URL) is who the backend says the reporter is: each report filed under
/// the token keeps it, shown to triage members and never to reporters.
/// Answers `{token, expires_at}`.
///
/// # Errors
/// [`ServiceCaller`]'s; 400 `invalid_args`, `invalid_origin`,
/// `invalid_reporter_id`, `invalid_key` or `invalid_profile`; 403
/// `wrong_origin` for an origin the queue does not list.
pub async fn mint_token(
    State(s): State<AppState>,
    c: ServiceCaller,
) -> Result<Json<ReporterToken>, ApiError> {
    let b: ReporterTokenRequest = json_body(&c.body)?;
    let profile = profile_of(b.profile)?;
    let grant = c.grant;
    let (token, expires_at) = s
        .store_call(move |st| {
            st.mint_reporter_token(
                &grant,
                &b.reporter_id,
                &b.origin,
                &b.public_key,
                profile.as_ref(),
            )
        })
        .await?;
    Ok(Json(ReporterToken {
        token: token.expose().to_string(),
        expires_at,
    }))
}

/// [`mint_token`]'s operation.
pub const MINT_TOKEN: Op = Op {
    id: "mint_reporter_token",
    handler: "mint_token",
    method: Method::Post,
    path: "/api/queues/{qid}/reporter-tokens",
    summary: "A service grant mints a reporter token for one reporter, origin and key.",
    query: None,
    request: Body::Json(schema::<ReporterTokenRequest>),
    status: 200,
    response: Body::Json(schema::<ReporterToken>),
    errors: &[
        (400, "invalid_args"),
        (400, "invalid_origin"),
        (400, "invalid_reporter_id"),
        (400, "invalid_key"),
        (400, "invalid_profile"),
        (403, "wrong_origin"),
    ],
    // A second token for the same key is as good as the first.
    retry: true,
    channel: &[Channel::Service],
    rights: Rights::Credential,
    stream: false,
};

/// A multipart body's parts by name, each at most once.
struct Parts(BTreeMap<String, Vec<u8>>);

/// Reads the parts of `mp`, each named in `allowed`; a part of another
/// name, or one given twice, is 400 `invalid_args`, its message naming
/// what `what` takes.
async fn read_parts(mut mp: Multipart, form: &[Part], what: &str) -> Result<Parts, ApiError> {
    let allowed: Vec<&str> = form.iter().map(|p| p.name).collect();
    let mut parts = BTreeMap::new();
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?
    {
        let name = field.name().unwrap_or("").to_string();
        if !allowed.contains(&name.as_str()) {
            let (last, rest) = allowed.split_last().expect("a body takes a part");
            return Err(ApiError::bad_request(
                "invalid_args",
                format!(
                    "unknown field '{name}': {what} takes {} and {last}",
                    rest.join(", ")
                ),
            ));
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|e| multipart_error(e.status(), e.body_text()))?;
        if parts.insert(name.clone(), bytes.to_vec()).is_some() {
            return Err(ApiError::bad_request(
                "invalid_args",
                format!("field '{name}' is given twice"),
            ));
        }
    }
    Ok(Parts(parts))
}

impl Parts {
    /// Part `name`'s bytes.
    fn bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        self.0.remove(name)
    }

    /// Part `name` as text; 400 `invalid_args` when it is not UTF-8.
    fn text(&mut self, name: &str) -> Result<Option<String>, ApiError> {
        self.bytes(name)
            .map(|b| {
                String::from_utf8(b).map_err(|_| {
                    ApiError::bad_request("invalid_args", format!("field '{name}' is not UTF-8"))
                })
            })
            .transpose()
    }

    /// Part `name` as text, which must be given.
    fn required(&mut self, name: &str) -> Result<String, ApiError> {
        self.text(name)?.ok_or_else(|| {
            ApiError::bad_request("invalid_args", format!("field '{name}' is required"))
        })
    }

    /// The `context` part, checked and scrubbed ([`parse_context`]).
    fn context(&mut self) -> Result<Option<ReportContext>, ApiError> {
        Ok(self
            .text("context")?
            .map(|c| parse_context(&c))
            .transpose()?)
    }

    /// The `diagnostics` part, checked and scrubbed ([`parse_diagnostics`]),
    /// a note stamped `now` in place of any line it dropped.
    fn diagnostics(&mut self, now: &str) -> Result<Option<Vec<DiagLine>>, ApiError> {
        Ok(self
            .bytes("diagnostics")
            .map(|d| parse_diagnostics(&d, now))
            .transpose()?)
    }

    /// The `clip` part, a PNG [`clip_problem`] takes; 400 `invalid_clip`
    /// otherwise.
    fn clip(&mut self) -> Result<Option<Vec<u8>>, ApiError> {
        let clip = self.bytes("clip");
        if let Some(problem) = clip.as_deref().and_then(clip_problem) {
            return Err(ApiError::bad_request("invalid_clip", problem));
        }
        Ok(clip)
    }
}

const fn text(name: &'static str, required: bool, description: &'static str) -> Part {
    Part {
        name,
        required,
        media_type: "text/plain",
        json: None,
        description,
    }
}

const CLIP_PART: Part = Part {
    name: "clip",
    required: false,
    media_type: "image/png",
    json: None,
    description: "The capture's PNG.",
};
const CONTEXT_PART: Part = Part {
    name: "context",
    required: false,
    media_type: "application/json",
    json: Some(schema::<ReportContext>),
    description: "What the reporter's browser said at capture: at most 16 KiB as compact JSON.",
};
const DIAGNOSTICS_PART: Part = Part {
    name: "diagnostics",
    required: false,
    media_type: "application/jsonl",
    json: Some(schema::<DiagLine>),
    description: "The page's recent errors: a DiagLine of JSON on each line.",
};

/// The parts a filing takes.
const REPORT_PARTS: &[Part] = &[
    text(
        "external_id",
        false,
        "The external artifact's ID (1 to 512 bytes).",
    ),
    text("url", true, "The page the report is on."),
    text("title", true, "The page's title."),
    Part {
        name: "anchor",
        required: true,
        media_type: "application/json",
        json: Some(schema::<Anchor>),
        description: "What on the page the report points at.",
    },
    text("body", true, "The report's first comment."),
    Part {
        name: "metadata",
        required: false,
        media_type: "application/json",
        json: None,
        description: "Opaque JSON the host app keeps with the report.",
    },
    text(
        "revision",
        false,
        "The external artifact's revision, as the host app names it.",
    ),
    CLIP_PART,
    CONTEXT_PART,
    DIAGNOSTICS_PART,
];

/// The parts a reopen takes.
const REOPEN_PARTS: &[Part] = &[
    text("body", true, "The reopen's comment."),
    CLIP_PART,
    DIAGNOSTICS_PART,
    CONTEXT_PART,
];

/// What this server says about itself on a report it receives: its
/// version, and the commit and build `/healthz` reports
/// ([`super::health::stamped`]), at the store's now.
fn server_stamp(s: &AppState) -> ServerStamp {
    let (commit, build) = super::health::stamped(s);
    ServerStamp {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: commit.map(str::to_string),
        build: build.map(str::to_string),
        received_at: s.store.now(),
    }
}

/// The report `p` describes, received by `s`.
fn new_report(s: &AppState, mut p: Parts, reporter_id: &str) -> Result<NewReport, ApiError> {
    let anchor: Anchor = serde_json::from_str(&p.required("anchor")?)
        .map_err(|e| ApiError::bad_request("invalid_anchor", e.to_string()))?;
    let metadata = match p.text("metadata")? {
        None => Value::Null,
        // Its size is the store's to check, as compact JSON.
        Some(m) => serde_json::from_str(&m)
            .map_err(|e| ApiError::bad_request("invalid_metadata", e.to_string()))?,
    };
    let clip = p.clip()?;
    let context = p.context()?;
    let diagnostics = p.diagnostics(&s.store.now())?;
    Ok(NewReport {
        external_id: p.text("external_id")?,
        url: p.required("url")?,
        title: p.required("title")?,
        anchor,
        body: p.required("body")?,
        author_name: reporter_id.to_string(),
        metadata,
        revision: p.text("revision")?,
        clip,
        context,
        server: Some(server_stamp(s)),
        diagnostics,
    })
}

/// Broadcasts report `tid`'s `thread` event, as a thread's routes do; a
/// failure is logged, the change being made.
async fn announce(s: &AppState, tid: String) {
    let ctx = s.feedback_ctx();
    let r = s
        .store_call(move |st| {
            if let Some(t) = st.get_thread(&tid)? {
                super::threads::publish_thread(&ctx, st, &t);
            }
            Ok(())
        })
        .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, "announcing a report failed");
    }
}

/// Wakes the sessions a filing or reopen woke (`woken`: the queue's
/// watchers and a reopen's previous proposer), their waits and pushes; a
/// failure is logged, the rows being written.
pub(crate) async fn wake_watchers(s: &AppState, woken: clax_core::feedback::Touched) {
    if woken.is_empty() {
        return;
    }
    let ctx = s.feedback_ctx();
    let r = s
        .store_call(move |st| {
            clax_realtime::feedback::apply(&ctx, st, &woken);
            Ok(())
        })
        .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, "waking a queue's watchers failed");
    }
}

/// `POST /api/queues/<qid>/reporter/reports` (multipart `external_id?`,
/// `url`, `title`, `anchor` (JSON), `body`, `metadata?` (JSON, at most
/// [`clax_core::queue::MAX_THREAD_METADATA_BYTES`] bytes written compact),
/// `revision?`, `clip?` (PNG), `context?` (JSON, the report context of
/// spec 2026-10-29-feedback-loop-design §5.1), `diagnostics?` (JSON lines,
/// `diagnostics.jsonl`, §5.2)): files a report as the token's reporter.
/// The context and diagnostics are checked and scrubbed again here, and
/// the report carries this server's stamp (`server: {version, commit?,
/// build?, received_at}`). Every session watching the queue is woken
/// (`POST /api/queues/<qid>/watch`). Answers `201 {report}`.
///
/// # Errors
/// [`ReporterCaller`]'s; 400 `invalid_args`, `invalid_multipart`,
/// `invalid_anchor`, `invalid_metadata`, `metadata_too_large`,
/// `invalid_clip`, `invalid_context`, `invalid_diagnostics` and the
/// store's refusals of bad input.
pub async fn file_report(
    State(s): State<AppState>,
    Path(qid): Path<String>,
    r: ReporterCaller<REPORT_BODY_LIMIT>,
) -> Result<(StatusCode, Json<ReportAnswer>), ApiError> {
    let mp = Multipart::from_request(r.request(), &())
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?;
    let token = r.token.clone();
    let rid = token.reporter_id.clone();
    let parts = read_parts(mp, REPORT_PARTS, "a report").await?;
    let new = new_report(&s, parts, &rid)?;
    // Each account of a hosted Clax is its own deployer, with its own
    // external IDs (spec §6.1).
    let scope = match s.hosted {
        Some(_) => ExternalScope::Maker,
        None => ExternalScope::Instance,
    };
    let (report, woken) = s
        .store_call_async(move |st| async move {
            st.file_report_touched_async(&reporter_ctx(&qid, &rid), &qid, Some(&token), scope, new)
                .await
        })
        .await?;
    announce(&s, report.id.clone()).await;
    wake_watchers(&s, woken).await;
    Ok((
        StatusCode::CREATED,
        Json(ReportAnswer {
            report: report.for_reporter(),
        }),
    ))
}

/// [`file_report`]'s operation.
pub const FILE_REPORT: Op = Op {
    id: "file_report",
    handler: "file_report",
    method: Method::Post,
    path: "/api/queues/{qid}/reporter/reports",
    summary: "The token's reporter files a report on an external artifact.",
    query: None,
    request: Body::Multipart(REPORT_PARTS),
    status: 201,
    response: Body::Json(schema::<ReportAnswer>),
    errors: &[
        (400, "invalid_args"),
        (400, "invalid_multipart"),
        (400, "invalid_anchor"),
        (400, "invalid_metadata"),
        (400, "metadata_too_large"),
        (400, "invalid_clip"),
        (400, "invalid_context"),
        (400, "invalid_diagnostics"),
        (400, "invalid_external_id"),
        (400, "external_artifact"),
    ],
    retry: false,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// `GET /api/queues/<qid>/reporter/reports?cursor=`: a page of the reports
/// the token's reporter sees, in order of arrival. Answers `{reports,
/// next}`.
///
/// # Errors
/// [`ReporterCaller`]'s; 400 `invalid_query` for another query.
pub async fn my_reports(
    State(s): State<AppState>,
    Path(qid): Path<String>,
    q: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
    r: ReporterCaller,
) -> Result<Json<ReportPage>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let rid = r.token.reporter_id;
    let (reports, next) = s
        .store_call(move |st| {
            st.reports(&qid, Visibility::Reporter(&rid), None, q.cursor.as_deref())
        })
        .await?;
    let reports: Vec<_> = reports
        .into_iter()
        .map(clax_core::queue::Report::for_reporter)
        .collect();
    Ok(Json(ReportPage { reports, next }))
}

/// [`my_reports`]'s operation.
pub const MY_REPORTS: Op = Op {
    id: "my_reports",
    handler: "my_reports",
    method: Method::Get,
    path: "/api/queues/{qid}/reporter/reports",
    summary: "A page of the reports the token's reporter sees, in order of arrival.",
    query: Some(schema::<PageQuery>),
    request: Body::Empty,
    status: 200,
    response: Body::Json(schema::<ReportPage>),
    errors: &[(400, "invalid_query")],
    retry: true,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// `POST /api/queues/<qid>/reporter/reports/<tid>/comments {body}`: the
/// token's reporter comments on a report it sees; the report's state is
/// left as it is (a closed report is reopened by `…/reopen` alone).
/// Answers `{report}`.
///
/// # Errors
/// [`ReporterCaller`]'s; 404 `not_found` for a report the reporter does
/// not see; 400 `invalid_args` or `invalid_comment`.
pub async fn comment(
    State(s): State<AppState>,
    Path((qid, tid)): Path<(String, String)>,
    r: ReporterCaller,
) -> Result<Json<ReportAnswer>, ApiError> {
    let b: CommentRequest = json_body(&r.body)?;
    let rid = r.token.reporter_id;
    let t = tid.clone();
    let (report, woken) = s
        .store_call(move |st| {
            st.reporter_comment_touched(&reporter_ctx(&qid, &rid), &qid, &t, &rid, &b.body)
        })
        .await?;
    announce(&s, tid).await;
    wake_watchers(&s, woken).await;
    Ok(Json(ReportAnswer {
        report: report.for_reporter(),
    }))
}

/// [`comment`]'s operation.
pub const COMMENT: Op = Op {
    id: "reporter_comment",
    handler: "comment",
    method: Method::Post,
    path: "/api/queues/{qid}/reporter/reports/{tid}/comments",
    summary: "The token's reporter comments on a report it sees.",
    query: None,
    request: Body::Json(schema::<CommentRequest>),
    status: 200,
    response: Body::Json(schema::<ReportAnswer>),
    errors: &[UNSEEN, (400, "invalid_args"), (400, "invalid_comment")],
    retry: false,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// A reporter's page keeps no clip: its token is short-lived and its
/// browser may be shared.
const REPORTER_CLIP_CACHE: &str = "private, no-store";

/// `GET /api/queues/<qid>/reporter/reports/<tid>/clip`: the clip of a
/// report the token's reporter sees, as `image/png`.
///
/// # Errors
/// [`ReporterCaller`]'s; 404 `not_found` for no clip, or a report the
/// reporter does not see.
pub async fn clip(
    State(s): State<AppState>,
    Path((qid, tid)): Path<(String, String)>,
    r: ReporterCaller,
) -> Result<Response, ApiError> {
    let rid = r.token.reporter_id;
    let bytes = s
        .store_call_async(move |st| async move {
            st.report_clip_async(&qid, &tid, Visibility::Reporter(&rid))
                .await?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(png(bytes, REPORTER_CLIP_CACHE))
}

/// [`clip`]'s operation.
pub const CLIP: Op = Op {
    id: "reporter_clip",
    handler: "clip",
    method: Method::Get,
    path: "/api/queues/{qid}/reporter/reports/{tid}/clip",
    summary: "The clip of a report the token's reporter sees.",
    query: None,
    request: Body::Empty,
    status: 200,
    response: Body::Png,
    errors: &[UNSEEN],
    retry: true,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// `GET /api/queues/<qid>/reporter/reports/<tid>/comments/<cid>/clip`: the
/// clip a reopen's comment carries, on a report the token's reporter sees,
/// as `image/png`.
///
/// # Errors
/// [`ReporterCaller`]'s; 404 `not_found` for no such comment or no clip on
/// it, or a report the reporter does not see.
pub async fn comment_clip(
    State(s): State<AppState>,
    Path((qid, tid, cid)): Path<(String, String, String)>,
    r: ReporterCaller,
) -> Result<Response, ApiError> {
    let rid = r.token.reporter_id;
    let bytes = s
        .store_call_async(move |st| async move {
            st.report_comment_clip_async(&qid, &tid, &cid, Visibility::Reporter(&rid))
                .await?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(png(bytes, REPORTER_CLIP_CACHE))
}

/// [`comment_clip`]'s operation.
pub const COMMENT_CLIP: Op = Op {
    id: "reporter_comment_clip",
    handler: "comment_clip",
    method: Method::Get,
    path: "/api/queues/{qid}/reporter/reports/{tid}/comments/{cid}/clip",
    summary: "The clip a reopen's comment carries, on a report the token's reporter sees.",
    query: None,
    request: Body::Empty,
    status: 200,
    response: Body::Png,
    errors: &[UNSEEN],
    retry: true,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// `POST /api/queues/<qid>/reporter/reports/<tid>/verify {note?,
/// context?}` (an empty body is `{}`): the token's reporter says the fix waiting on a report it
/// filed works (spec 2026-10-29-feedback-loop-design §6.3), with its
/// browser's `context` at verification (the filing's shape, checked and
/// scrubbed the same way). The report is `verified`, resolved for reason
/// `fixed`. Answers `{report}`.
///
/// # Errors
/// [`ReporterCaller`]'s; 404 `not_found` for a report the reporter did not
/// file; 409 `not_fix_proposed` when no fix waits for a verdict; 400
/// `invalid_args`, `invalid_comment` or `invalid_context`.
pub async fn verify(
    State(s): State<AppState>,
    Path((qid, tid)): Path<(String, String)>,
    r: ReporterCaller,
) -> Result<Json<ReportAnswer>, ApiError> {
    // An empty body names nothing, as `{}` does.
    let b: VerifyRequest = if r.body.iter().all(u8::is_ascii_whitespace) {
        VerifyRequest::default()
    } else {
        json_body(&r.body)?
    };
    let context = match b.context {
        None | Some(Value::Null) => None,
        Some(c) => Some(parse_context(&c.to_string())?),
    };
    let rid = r.token.reporter_id;
    let an = Announcer::of(&s);
    let report = s
        .store_call(move |st| {
            let ctx = reporter_ctx(&qid, &rid);
            let (report, touched) = st.verify_report_touched(
                &ctx,
                &qid,
                &tid,
                Verdict::Reporter(&rid),
                b.note,
                context,
            )?;
            let t = st.get_thread(&tid)?.ok_or(CoreError::NotFound)?;
            announce_resolved(&an, st, &ctx, &t, None, &touched);
            Ok(report)
        })
        .await?;
    Ok(Json(ReportAnswer {
        report: report.for_reporter(),
    }))
}

/// [`verify`]'s operation.
pub const VERIFY: Op = Op {
    id: "reporter_verify",
    handler: "verify",
    method: Method::Post,
    path: "/api/queues/{qid}/reporter/reports/{tid}/verify",
    summary: "The token's reporter says the fix waiting on a report it filed works.",
    query: None,
    request: Body::Json(schema::<VerifyRequest>),
    status: 200,
    response: Body::Json(schema::<ReportAnswer>),
    errors: &[
        UNSEEN,
        (409, "not_fix_proposed"),
        (400, "invalid_args"),
        (400, "invalid_comment"),
        (400, "invalid_context"),
    ],
    // Sent again after it took, it is 409 `not_fix_proposed`.
    retry: false,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// `POST /api/queues/<qid>/reporter/reports/<tid>/reopen` (multipart
/// `body`, `clip?` (PNG), `diagnostics?` (JSON lines), `context?` (JSON)):
/// the token's reporter says a report it filed is still broken (spec
/// 2026-10-29-feedback-loop-design §6.3), from `fix_proposed`, `verified`
/// or `resolved`. The reopen is a new capture: `body` is its comment, which
/// carries the capture's number, its clip (`…/comments/<cid>/clip`) and its
/// diagnostics (`…/diagnostics?capture=<n>`); the filing's stay as they
/// were. A fix waiting is `rejected`, and the report is `open` again with
/// `reopen_count` one more. The queue's watching sessions and the agent's
/// session that proposed the fix the reopen rejects are woken. Answers
/// `{report}`.
///
/// # Errors
/// [`ReporterCaller`]'s; 404 `not_found` for a report the reporter did not
/// file; 409 `not_fix_proposed` for an `open` or `claimed` report; 400
/// `invalid_args`, `invalid_multipart`, `invalid_comment`, `invalid_clip`,
/// `invalid_diagnostics` or `invalid_context`.
pub async fn reopen(
    State(s): State<AppState>,
    Path((qid, tid)): Path<(String, String)>,
    r: ReporterCaller<REPORT_BODY_LIMIT>,
) -> Result<Json<ReportAnswer>, ApiError> {
    let mp = Multipart::from_request(r.request(), &())
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?;
    let mut p = read_parts(mp, REOPEN_PARTS, "a reopen").await?;
    let body = p.required("body")?;
    let clip = p.clip()?;
    let diagnostics = p.diagnostics(&s.store.now())?;
    let context = p.context()?;
    let rid = r.token.reporter_id;
    let t = tid.clone();
    let (report, woken) = s
        .store_call_async(move |st| async move {
            st.reopen_report_async(
                &reporter_ctx(&qid, &rid),
                &qid,
                &t,
                Verdict::Reporter(&rid),
                body,
                clip,
                diagnostics,
                context,
            )
            .await
        })
        .await?;
    wake_watchers(&s, woken).await;
    let cid = last_comment(&report);
    let an = Announcer::of(&s);
    let done = s
        .store_call(move |st| announce_comment(&an, st, &tid, cid.as_deref()))
        .await;
    if let Err(e) = done {
        tracing::warn!(error = ?e, "announcing a reopened report failed");
    }
    Ok(Json(ReportAnswer {
        report: report.for_reporter(),
    }))
}

/// [`reopen`]'s operation.
pub const REOPEN: Op = Op {
    id: "reporter_reopen",
    handler: "reopen",
    method: Method::Post,
    path: "/api/queues/{qid}/reporter/reports/{tid}/reopen",
    summary: "The token's reporter says a report it filed is still broken: a new capture.",
    query: None,
    request: Body::Multipart(REOPEN_PARTS),
    status: 200,
    response: Body::Json(schema::<ReportAnswer>),
    errors: &[
        UNSEEN,
        (409, "not_fix_proposed"),
        (400, "invalid_args"),
        (400, "invalid_multipart"),
        (400, "invalid_comment"),
        (400, "invalid_clip"),
        (400, "invalid_diagnostics"),
        (400, "invalid_context"),
    ],
    retry: false,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// `GET /api/queues/<qid>/reporter/reports/<tid>/diagnostics?capture=`:
/// the diagnostics of a report the token's reporter filed: capture
/// `capture`'s (0 the filing's, `n` the `n`th reopen's), or without it the
/// latest capture's that has any. Answers `{lines}`, `[]` when that
/// capture has none. Another reporter's are never served, even on a
/// report `reporters_see: all` shows it.
///
/// # Errors
/// [`ReporterCaller`]'s; 404 `not_found` for a report the reporter did
/// not file; 400 `invalid_query` for another query.
pub async fn diagnostics(
    State(s): State<AppState>,
    Path((qid, tid)): Path<(String, String)>,
    q: Result<Query<CaptureQuery>, axum::extract::rejection::QueryRejection>,
    r: ReporterCaller,
) -> Result<Json<DiagnosticsAnswer>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let rid = r.token.reporter_id;
    let lines = s
        .store_call(move |st| {
            st.report_diagnostics(&qid, &tid, Visibility::Reporter(&rid), q.capture)
        })
        .await?;
    Ok(Json(DiagnosticsAnswer {
        lines: lines.unwrap_or_default(),
    }))
}

/// [`diagnostics`]'s operation.
pub const DIAGNOSTICS: Op = Op {
    id: "reporter_diagnostics",
    handler: "diagnostics",
    method: Method::Get,
    path: "/api/queues/{qid}/reporter/reports/{tid}/diagnostics",
    summary: "The diagnostics of one capture of a report the token's reporter filed.",
    query: Some(schema::<CaptureQuery>),
    request: Body::Empty,
    status: 200,
    response: Body::Json(schema::<DiagnosticsAnswer>),
    errors: &[UNSEEN, (400, "invalid_query")],
    retry: true,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// `POST /api/queues/<qid>/reporter/reports/<tid>/seen`: the token's
/// reporter has read a report it sees; a comment before now no longer
/// counts in its badge. Answers 204.
///
/// # Errors
/// [`ReporterCaller`]'s; 404 `not_found` for a report the reporter does
/// not see.
pub async fn seen(
    State(s): State<AppState>,
    Path((qid, tid)): Path<(String, String)>,
    r: ReporterCaller,
) -> Result<StatusCode, ApiError> {
    let rid = r.token.reporter_id;
    s.store_call(move |st| st.mark_reporter_seen(&qid, &tid, &rid))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// [`seen`]'s operation.
pub const SEEN: Op = Op {
    id: "reporter_seen",
    handler: "seen",
    method: Method::Post,
    path: "/api/queues/{qid}/reporter/reports/{tid}/seen",
    summary: "The token's reporter has read a report it sees.",
    query: None,
    request: Body::Empty,
    status: 204,
    response: Body::Empty,
    errors: &[UNSEEN],
    retry: true,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// `GET /api/queues/<qid>/reporter/badge`: what waits for the token's
/// reporter: the reports it filed whose fix waits for its verdict, or that
/// hold a comment not its own since it last read them (`…/seen`), oldest
/// first. Answers `{waiting, report_ids}`.
///
/// # Errors
/// [`ReporterCaller`]'s.
pub async fn badge(
    State(s): State<AppState>,
    Path(qid): Path<String>,
    r: ReporterCaller,
) -> Result<Json<Badge>, ApiError> {
    let rid = r.token.reporter_id;
    let badge = s
        .store_call(move |st| st.reporter_badge(&qid, &rid))
        .await?;
    Ok(Json(badge))
}

/// [`badge`]'s operation.
pub const BADGE: Op = Op {
    id: "reporter_badge",
    handler: "badge",
    method: Method::Get,
    path: "/api/queues/{qid}/reporter/badge",
    summary: "What waits for the token's reporter.",
    query: None,
    request: Body::Empty,
    status: 200,
    response: Body::Json(schema::<Badge>),
    errors: &[],
    retry: true,
    channel: REPORTER,
    rights: Rights::Credential,
    stream: false,
};

/// `OPTIONS` on each reporter route: the CORS preflight, `204`, whose
/// headers [`reporter::cors`] adds for the queue's origins.
pub async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These routes run before every credential layer of the daemon, so
    /// each handler but the preflight takes the credential extractor that
    /// authenticates it, and every reporter route has its CORS layer.
    #[test]
    fn every_handler_takes_its_credential() {
        let src = include_str!("queue_reporter.rs");
        let src = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        let mut seen = 0;
        for part in src.split("\npub async fn ").skip(1) {
            let name = part.split('(').next().unwrap();
            let sig = &part[..part.find('{').unwrap()];
            seen += 1;
            match name {
                "preflight" => assert!(sig.contains("()"), "{name}"),
                "mint_token" => assert!(sig.contains("c: ServiceCaller"), "{name}"),
                _ => assert!(sig.contains("r: ReporterCaller"), "{name}"),
            }
        }
        assert_eq!(seen, 12);
        let routes = &src[src.find("pub fn router").unwrap()..];
        let routes = &routes[..routes.find("\n}\n").unwrap()];
        assert_eq!(routes.matches(".route(").count(), 10);
        assert_eq!(routes.matches(".route_layer(").count(), 1);
        assert!(routes.find("reporter::cors").unwrap() < routes.find("reporter-tokens").unwrap());
    }

    /// Every route of this router is public: each method of each route is
    /// one of [`CONTRACT`]'s operations, by path, method and handler, and
    /// each operation's channel is the credential its handler takes.
    #[test]
    fn every_route_is_in_the_contract() {
        let src = include_str!("queue_reporter.rs");
        let src = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        let routes = &src[src.find("pub fn router").unwrap()..];
        let routes = &routes[..routes.find("\n}\n").unwrap()];
        let routed = crate::contract::routed(routes);
        let mut declared: Vec<_> = CONTRACT
            .iter()
            .map(|op| {
                (
                    op.path.to_string(),
                    op.method.as_str(),
                    op.handler.to_string(),
                )
            })
            .collect();
        declared.sort();
        let mut routed: Vec<_> = routed
            .into_iter()
            .filter(|(_, _, h)| h != "preflight")
            .collect();
        routed.sort();
        assert_eq!(routed, declared);
        for op in CONTRACT {
            let at = src
                .find(&format!("\npub async fn {}(", op.handler))
                .unwrap_or_else(|| panic!("no handler {}", op.handler));
            let sig = &src[at..at + src[at..].find('{').unwrap()];
            let want = match op.channel {
                [Channel::Reporter] => "ReporterCaller",
                [Channel::Service] => "ServiceCaller",
                other => panic!("{}: a reporter route's channel is one: {other:?}", op.id),
            };
            assert!(sig.contains(want), "{} takes {want}", op.handler);
        }
    }

    /// A multipart body's required parts are the ones its reader requires.
    #[test]
    fn the_forms_parts_are_the_readers() {
        let src = include_str!("queue_reporter.rs");
        let read = |f: &str| {
            let at = src.find(&format!("fn {f}(")).unwrap();
            let body = &src[at..at + src[at..].find("\n}\n").unwrap()];
            let mut names: Vec<String> = body
                .split("p.required(\"")
                .skip(1)
                .map(|s| s[..s.find('"').unwrap()].to_string())
                .collect();
            names.sort();
            names
        };
        let required = |form: &[Part]| {
            let mut names: Vec<String> = form
                .iter()
                .filter(|p| p.required)
                .map(|p| p.name.to_string())
                .collect();
            names.sort();
            names
        };
        assert_eq!(required(REPORT_PARTS), read("new_report"));
        assert_eq!(required(REOPEN_PARTS), read("reopen"));
    }
}

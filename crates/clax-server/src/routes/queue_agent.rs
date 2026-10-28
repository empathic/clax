//! A queue's reports for its triage members and their agents (spec
//! 2026-10-27-clax-sdk-design §7.3, §7.4), through `Agent<N>`:
//!
//! - `GET /api/queues/<qid>/reports?state=&cursor=` → `{reports, next}`;
//!   `GET /api/queues/<qid>/reports/<tid>` → `{report}`.
//! - `POST …/reports/<tid>/claim`, `…/renew` → a lease; `…/release`.
//! - `POST …/reports/<tid>/comments {body}`, `POST …/reports/<tid>/resolve`.
//! - `PATCH …/reports/<tid> {metadata, if_version}` (the thread's),
//!   `PATCH …/reports/<tid>/artifact {metadata, if_version}` (its external
//!   artifact's).
//! - `GET …/reports/<tid>/clip`.
//!
//! Each answers 501 `not_yet` until it is built.

use super::queues::not_yet;
use axum::http::StatusCode;
use clax_core::audit::AuditCtx;
use clax_http::agent::{
    Agent, QueueArtifactPatch, QueueClaim, QueueClip, QueueRelease, QueueRenew, QueueReply,
    QueueReportGet, QueueReportPatch, QueueReportsList, QueueResolve,
};
use clax_http::error::ApiError;

/// `GET /api/queues/<qid>/reports`.
pub async fn list(_a: Agent<QueueReportsList>) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `GET /api/queues/<qid>/reports/<tid>`.
pub async fn get(_a: Agent<QueueReportGet>) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `POST /api/queues/<qid>/reports/<tid>/claim`.
pub async fn claim(_a: Agent<QueueClaim>, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `POST /api/queues/<qid>/reports/<tid>/renew`.
pub async fn renew(_a: Agent<QueueRenew>) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `POST /api/queues/<qid>/reports/<tid>/release`.
pub async fn release(_a: Agent<QueueRelease>, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `POST /api/queues/<qid>/reports/<tid>/comments`.
pub async fn reply(_a: Agent<QueueReply>, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `POST /api/queues/<qid>/reports/<tid>/resolve`.
pub async fn resolve(_a: Agent<QueueResolve>, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `PATCH /api/queues/<qid>/reports/<tid>`.
pub async fn patch(_a: Agent<QueueReportPatch>, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `PATCH /api/queues/<qid>/reports/<tid>/artifact`.
pub async fn patch_artifact(
    _a: Agent<QueueArtifactPatch>,
    _ctx: AuditCtx,
) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `GET /api/queues/<qid>/reports/<tid>/clip`.
pub async fn clip(_a: Agent<QueueClip>) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

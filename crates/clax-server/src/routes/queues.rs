//! Report queues' administration (spec 2026-10-27-clax-sdk-design §7.1):
//! a queue's maker and its admins, in a browser session, make and change
//! queues, their members and their service grants.
//!
//! - `GET /api/queues` → `{queues}`: the caller's queues with their role
//!   (also an agent's, for `queue_list`).
//! - `POST /api/queues {name, origins, reporters_see}` → `{queue}`;
//!   `PATCH /api/queues/<qid>`, `DELETE /api/queues/<qid>`.
//! - `GET /api/queues/<qid>/members` → `{members}`; `PUT
//!   /api/queues/<qid>/members/<public_id> {role}`, `DELETE` the same.
//! - `GET /api/queues/<qid>/service-grants` → `{grants}`; `POST
//!   /api/queues/<qid>/service-grants {name}` → `{grant, token,
//!   private_key}`, shown once; `DELETE
//!   /api/queues/<qid>/service-grants/<gid>`.
//!
//! Each answers 501 `not_yet` until it is built.

use axum::http::StatusCode;
use clax_core::audit::AuditCtx;
use clax_http::agent::{Agent, DeniedToGrants, QueuesList};
use clax_http::error::ApiError;

/// 501 `not_yet`: the route is declared and not yet built.
pub(crate) fn not_yet() -> ApiError {
    ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "not_yet",
        "this queue route is not built yet",
    )
}

/// `GET /api/queues`.
pub async fn list(_a: Agent<QueuesList>) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `POST /api/queues`.
pub async fn create(_d: DeniedToGrants, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `PATCH /api/queues/<qid>`.
pub async fn patch(_d: DeniedToGrants, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `DELETE /api/queues/<qid>`.
pub async fn delete(_d: DeniedToGrants, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `GET /api/queues/<qid>/members`.
pub async fn members(_d: DeniedToGrants) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `PUT /api/queues/<qid>/members/<public_id>`.
pub async fn put_member(_d: DeniedToGrants, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `DELETE /api/queues/<qid>/members/<public_id>`.
pub async fn delete_member(_d: DeniedToGrants, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `GET /api/queues/<qid>/service-grants`.
pub async fn grants(_d: DeniedToGrants) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `POST /api/queues/<qid>/service-grants`.
pub async fn create_grant(_d: DeniedToGrants, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

/// `DELETE /api/queues/<qid>/service-grants/<gid>`.
pub async fn revoke_grant(_d: DeniedToGrants, _ctx: AuditCtx) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

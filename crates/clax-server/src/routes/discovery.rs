//! `GET /.well-known/clax` (spec 2026-10-27-clax-sdk-design §4.2): what
//! this Clax is (`local`, `hosted` or `worker`), the contract version, its
//! sign-in methods and capabilities, for any caller with no credential.
//!
//! Answers 501 `not_yet` until it is built.

use super::queues::not_yet;
use axum::http::StatusCode;
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;

/// `GET /.well-known/clax`.
pub async fn get(_d: DeniedToGrants) -> Result<StatusCode, ApiError> {
    Err(not_yet())
}

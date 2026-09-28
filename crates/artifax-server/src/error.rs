//! JSON error responses: `{"error": {"code", "message", ...}}`.

use artifax_core::CoreError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            code,
            message: message.into(),
            extra: Default::default(),
        }
    }
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid bearer token is required",
        )
    }
    pub fn forbidden(code: &'static str, msg: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, msg)
    }
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }
    pub fn bad_request(code: &'static str, msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, msg)
    }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::NotFound => ApiError::not_found(),
            CoreError::Conflict { current } => {
                let mut err = ApiError::new(
                    StatusCode::CONFLICT,
                    "conflict",
                    format!("artifact is at version {current}"),
                );
                err.extra.insert("current".into(), json!(current));
                err
            }
            CoreError::Invalid { code, message } => ApiError::bad_request(code, message),
            CoreError::Io(e) => {
                tracing::error!(error = %e, "io");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "storage error",
                )
            }
            CoreError::Db(e) => {
                tracing::error!(error = %e, "db");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "database error",
                )
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut err = serde_json::Map::new();
        err.insert("code".into(), json!(self.code));
        err.insert("message".into(), json!(self.message));
        err.extend(self.extra);
        (self.status, axum::Json(json!({"error": err}))).into_response()
    }
}

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
            e @ CoreError::Corrupt { .. } => {
                tracing::error!(error = %e, "corrupt row");
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "corrupt", e.to_string())
            }
            // The kind (such as `StorageFull`) tells a caller whether a retry
            // can help; the message, which may name paths, stays in the log.
            CoreError::Io(e) => {
                tracing::error!(error = %e, "io");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    format!("storage error: {:?}", e.kind()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_name_their_kind_but_no_path() {
        let e = std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "/home/me/.artifax/blobs/ab: no space left on device",
        );
        let api = ApiError::from(CoreError::Io(e));
        assert_eq!(api.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(api.code, "internal");
        assert_eq!(api.message, "storage error: StorageFull");
    }
}

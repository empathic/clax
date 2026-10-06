//! JSON error responses: `{"error": {"code", "message", ...}}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use clax_core::CoreError;
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
            CoreError::DocNotFound { path } => {
                let mut err = ApiError::not_found();
                err.extra.insert("path".into(), json!(path));
                err
            }
            CoreError::Conflict { current } => {
                let mut err = ApiError::new(
                    StatusCode::CONFLICT,
                    "conflict",
                    format!("artifact is at version {current}"),
                );
                err.extra.insert("current".into(), json!(current));
                err
            }
            CoreError::DocConflict { path, current } => {
                let mut err = ApiError::new(
                    StatusCode::CONFLICT,
                    "conflict",
                    match current {
                        Some(n) => format!(
                            "document {path} is at version {n}; re-read it and redo the write"
                        ),
                        None => format!("document {path} does not exist"),
                    },
                );
                err.extra.insert("path".into(), json!(path));
                err.extra.insert("current".into(), json!(current));
                err
            }
            CoreError::DocPinRequired { path, current } => {
                let mut err = ApiError::bad_request(
                    "if_version_required",
                    format!(
                        "document {path} exists at version {current}; read it and pass its version as if_version"
                    ),
                );
                err.extra.insert("path".into(), json!(path));
                err.extra.insert("current".into(), json!(current));
                err
            }
            CoreError::InBatch { op, path, error } => {
                let mut err = ApiError::from(*error);
                err.message = format!("batch write {op} ({path}): {}", err.message);
                err.extra.insert("op".into(), json!(op));
                err.extra.insert("path".into(), json!(path));
                err
            }
            CoreError::Invalid {
                code: "nothing_to_send",
                message,
            } => ApiError::new(StatusCode::CONFLICT, "nothing_to_send", message),
            CoreError::Invalid { code, message } => ApiError::bad_request(code, message),
            e @ CoreError::NotDeclared { .. } => ApiError::forbidden("not_declared", e.to_string()),
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
            e @ CoreError::ReadTimeout => {
                tracing::warn!(error = %e, "read interrupted");
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "read_timeout",
                    e.to_string(),
                )
            }
            e @ CoreError::SchemaNewer { .. } => {
                tracing::error!(error = %e, "schema newer than this binary");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "schema_newer",
                    e.to_string(),
                )
            }
            CoreError::TaskFailed => ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "storage task failed",
            ),
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
            "/home/me/.clax/blobs/ab: no space left on device",
        );
        let api = ApiError::from(CoreError::Io(e));
        assert_eq!(api.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(api.code, "internal");
        assert_eq!(api.message, "storage error: StorageFull");
    }
    #[test]
    fn document_conflicts_name_the_path_and_current_version() {
        let e = ApiError::from(CoreError::DocConflict {
            path: "tasks/t1".into(),
            current: Some(3),
        });
        assert_eq!((e.status, e.code), (StatusCode::CONFLICT, "conflict"));
        assert_eq!(
            (e.extra["path"].as_str(), e.extra["current"].as_u64()),
            (Some("tasks/t1"), Some(3))
        );
        let e = ApiError::from(CoreError::DocConflict {
            path: "t/x".into(),
            current: None,
        });
        assert!(e.extra["current"].is_null());
        let e = ApiError::from(CoreError::DocPinRequired {
            path: "tasks/t1".into(),
            current: 2,
        });
        assert_eq!(
            (e.status, e.code, e.extra["current"].as_u64()),
            (StatusCode::BAD_REQUEST, "if_version_required", Some(2))
        );
    }

    #[test]
    fn batch_failures_carry_the_op_index_and_path() {
        let e = ApiError::from(CoreError::InBatch {
            op: 2,
            path: "t/x".into(),
            error: Box::new(CoreError::NotFound),
        });
        assert_eq!((e.status, e.code), (StatusCode::NOT_FOUND, "not_found"));
        assert_eq!(
            (e.extra["op"].as_u64(), e.extra["path"].as_str()),
            (Some(2), Some("t/x"))
        );
        let e = ApiError::from(CoreError::InBatch {
            op: 0,
            path: "t/1".into(),
            error: Box::new(CoreError::DocConflict {
                path: "t/1".into(),
                current: Some(4),
            }),
        });
        assert_eq!(
            (
                e.status,
                e.code,
                e.extra["current"].as_u64(),
                e.extra["op"].as_u64()
            ),
            (StatusCode::CONFLICT, "conflict", Some(4), Some(0))
        );
        let e = ApiError::from(CoreError::InBatch {
            op: 1,
            path: "odd".into(),
            error: Box::new(CoreError::invalid("invalid_argument", "bad path")),
        });
        assert_eq!(
            (e.status, e.code, e.extra["op"].as_u64()),
            (StatusCode::BAD_REQUEST, "invalid_argument", Some(1))
        );
        assert!(e.message.contains("bad path"), "{}", e.message);
    }
}

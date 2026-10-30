//! JSON error responses: `{"error": {"code", "message", ...}}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use clax_core::CoreError;
use serde_json::json;

/// The hint on every 404 of an artifact on the hosted instance at
/// `origin` ([`ApiError::not_found_artifact`]).
pub fn not_found_hint(origin: &str) -> String {
    format!(
        "This may be a private page. If it is yours, sign in at {origin}/signin. Agents: connect this Clax instance with the connect_instance tool or `clax remote request <url>`."
    )
}

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
    /// The 404 of an artifact on a hosted instance whose public origin is
    /// `origin`: [`ApiError::not_found`] with the one hint every such 404
    /// carries, so it tells nothing of whether the artifact exists. It
    /// names where to sign in (`sign_in`) and the instance's discovery
    /// document (`instance`).
    pub fn not_found_artifact(origin: &str) -> Self {
        let mut err = Self::not_found();
        err.extra
            .insert("hint".into(), json!(not_found_hint(origin)));
        err.extra
            .insert("sign_in".into(), json!(format!("{origin}/signin")));
        err.extra.insert(
            "instance".into(),
            json!(format!("{origin}/.well-known/clax")),
        );
        err
    }
    pub fn bad_request(code: &'static str, msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, msg)
    }
}

/// A sample body the checks refuse is a 400 with the check's code.
impl From<clax_sample::request::SampleError> for ApiError {
    fn from(e: clax_sample::request::SampleError) -> Self {
        ApiError::bad_request(e.code, e.message)
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
            // An approval of a version or declaration no longer current
            // (`Store::approve_capabilities`): review the current one.
            e @ CoreError::StaleVersion { current } => {
                let mut err = ApiError::new(StatusCode::CONFLICT, "stale_version", e.to_string());
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
            // The routes name the authors (`docs::str_replace`); a handler
            // that does not answers without them.
            CoreError::DocWithheld { .. } => ApiError::forbidden(
                "withheld",
                "this document is withheld until your person approves its authors",
            ),
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
            // ClaxBot refused a run before any model call (spec §6.4, §7.4).
            CoreError::Invalid {
                code:
                    code @ ("no_flag" | "credits_exhausted" | "not_configured" | "model_not_priced"),
                message,
            } => ApiError::new(StatusCode::FORBIDDEN, code, message),
            // A ClaxBot run already works on one of the threads sent
            // (`clax_server::routes::bot::run_active`): its ID is the
            // message, answered as `run: {id}`.
            CoreError::Invalid {
                code: "run_active",
                message,
            } => {
                let mut err = ApiError::new(
                    StatusCode::CONFLICT,
                    "run_active",
                    "a ClaxBot run is already working on this thread: follow it, or stop it first",
                );
                err.extra.insert("run".into(), json!({"id": message}));
                err
            }
            // A right the caller lacks, found inside a store job.
            CoreError::Invalid {
                code: "not_permitted",
                message,
            } => ApiError::new(StatusCode::FORBIDDEN, "not_permitted", message),
            // A ClaxBot setting the daemon's configuration sets.
            CoreError::Invalid {
                code: "setting_overridden",
                message,
            } => ApiError::new(StatusCode::CONFLICT, "setting_overridden", message),
            CoreError::Invalid {
                code: "nothing_to_send",
                message,
            } => ApiError::new(StatusCode::CONFLICT, "nothing_to_send", message),
            // A joined site's unfinished join or un-merge (spec §7.2): finish it first.
            CoreError::Invalid {
                code: code @ ("joining" | "unmerging"),
                message,
            } => ApiError::new(StatusCode::CONFLICT, code, message),
            // A repository token blind to a publish could not be revoked.
            CoreError::Invalid {
                code: "repo_unavailable",
                message,
            } => ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "repo_unavailable", message),
            // main ends in pushed commits whose pusher may not publish.
            CoreError::Invalid {
                code: "push_refused",
                message,
            } => ApiError::new(StatusCode::CONFLICT, "push_refused", message),
            // Unrecorded content holds the version (`docs/contract.md`).
            CoreError::Invalid {
                code: "version_present",
                message,
            } => ApiError::new(StatusCode::CONFLICT, "version_present", message),
            CoreError::Invalid {
                code: "question_closed",
                message,
            } => ApiError::new(StatusCode::CONFLICT, "question_closed", message),
            // The home's config.toml links to a file that does not exist.
            CoreError::Invalid {
                code: "config_symlink_dangling",
                message,
            } => ApiError::new(StatusCode::CONFLICT, "config_symlink_dangling", message),
            CoreError::Invalid {
                code: "limit_reached",
                message,
            } => ApiError::new(StatusCode::TOO_MANY_REQUESTS, "limit_reached", message),
            // The grant flow (spec §6.2–§6.4).
            CoreError::Invalid {
                code:
                    code @ ("name_taken" | "tag_exists" | "name_locked" | "key_in_use" | "not_pending"
                    | "already_issued" | "not_issued" | "owner_answered" | "answer_closed"
                    | "not_on_lan" | "request_closed"),
                message,
            } => ApiError::new(StatusCode::CONFLICT, code, message),
            CoreError::Invalid {
                code:
                    code @ ("invalid_token"
                    | "grant_revoked"
                    | "replayed_request"
                    | "invalid_session_proof"),
                message,
            } => ApiError::new(StatusCode::UNAUTHORIZED, code, message),
            CoreError::Invalid {
                code: "not_raised",
                message,
            } => ApiError::new(StatusCode::NOT_FOUND, "not_raised", message),
            CoreError::Invalid {
                code:
                    code @ ("denied" | "foreign_session" | "not_creator" | "beyond_your_rights"
                    | "quota_exceeded" | "github_blocked" | "github_not_owner" | "comments_off"
                    | "new_people_only"),
                message,
            } => ApiError::new(StatusCode::FORBIDDEN, code, message),
            // A cross-app read without consent.
            CoreError::Invalid {
                code: "not_granted",
                message,
            } => ApiError::forbidden("not_granted", message),
            // What a draft refuses.
            CoreError::Invalid {
                code: "draft",
                message,
            } => ApiError::forbidden("draft", message),
            CoreError::Invalid { code, message } => ApiError::bad_request(code, message),
            // The rule broken rides beside the sentence (`docs/contract.md`).
            CoreError::InvalidIcon(p) => {
                let mut err = ApiError::bad_request("invalid_icon", p.to_string());
                err.extra.insert("rule".into(), json!(p.code()));
                err
            }
            CoreError::ThreadTaken { name } => {
                let message = CoreError::ThreadTaken { name: name.clone() }.to_string();
                let mut err = ApiError::new(StatusCode::CONFLICT, "thread_taken", message);
                err.extra.insert("taken_by".into(), json!(name));
                err
            }
            // Report queues (spec 2026-10-27-clax-sdk-design §5.3, §6.2, §7.4).
            e @ CoreError::LeaseLost => {
                ApiError::new(StatusCode::CONFLICT, "lease_lost", e.to_string())
            }
            e @ CoreError::ClaimRequired => {
                ApiError::new(StatusCode::CONFLICT, "claim_required", e.to_string())
            }
            e @ CoreError::ExternalArtifact => {
                ApiError::bad_request("external_artifact", e.to_string())
            }
            e @ CoreError::WrongOrigin => ApiError::forbidden("wrong_origin", e.to_string()),
            e @ CoreError::TokenExpired => {
                ApiError::new(StatusCode::UNAUTHORIZED, "token_expired", e.to_string())
            }
            CoreError::QueueReport { queue_id } => {
                let message = CoreError::QueueReport {
                    queue_id: queue_id.clone(),
                }
                .to_string();
                let mut err = ApiError::new(StatusCode::CONFLICT, "queue_report", message);
                err.extra.insert("queue_id".into(), json!(queue_id));
                err
            }
            e @ CoreError::NotAMember => {
                ApiError::new(StatusCode::NOT_FOUND, "not_a_member", e.to_string())
            }
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
            e @ CoreError::MigrationFailed { .. } => {
                tracing::error!(error = %e, "database upgrade failed");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "migration_failed",
                    e.to_string(),
                )
            }
            CoreError::TaskFailed => ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "storage task failed",
            ),
            // A defect in the store's caller: logged, and answered as any
            // failed storage task.
            e @ CoreError::NestedJob { .. } => {
                tracing::error!(error = %e, "nested store job");
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "storage task failed",
                )
            }
            // Another request holds the artifact's content: a retry can help.
            e @ CoreError::WouldBlock { .. } => {
                tracing::warn!(error = %e, "store busy");
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "busy", e.to_string())
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
    fn queue_errors_answer_their_codes() {
        for (e, status, code) in [
            (CoreError::LeaseLost, StatusCode::CONFLICT, "lease_lost"),
            (
                CoreError::ClaimRequired,
                StatusCode::CONFLICT,
                "claim_required",
            ),
            (
                CoreError::ExternalArtifact,
                StatusCode::BAD_REQUEST,
                "external_artifact",
            ),
            (
                CoreError::WrongOrigin,
                StatusCode::FORBIDDEN,
                "wrong_origin",
            ),
            (
                CoreError::TokenExpired,
                StatusCode::UNAUTHORIZED,
                "token_expired",
            ),
            (CoreError::NotAMember, StatusCode::NOT_FOUND, "not_a_member"),
            (
                CoreError::QueueReport {
                    queue_id: "q1".into(),
                },
                StatusCode::CONFLICT,
                "queue_report",
            ),
        ] {
            let e = ApiError::from(e);
            assert_eq!((e.status, e.code), (status, code));
        }
    }

    #[test]
    fn closed_questions_conflict_and_limits_are_too_many() {
        let e = ApiError::from(CoreError::invalid(
            "question_closed",
            "the question is released",
        ));
        assert_eq!(
            (e.status, e.code),
            (StatusCode::CONFLICT, "question_closed")
        );
        let e = ApiError::from(CoreError::invalid("limit_reached", "at most 8"));
        assert_eq!(
            (e.status, e.code),
            (StatusCode::TOO_MANY_REQUESTS, "limit_reached")
        );
        let e = ApiError::from(CoreError::invalid("version_present", "version 6 holds"));
        assert_eq!(
            (e.status, e.code),
            (StatusCode::CONFLICT, "version_present")
        );
        let e = ApiError::from(CoreError::StaleVersion { current: 5 });
        assert_eq!((e.status, e.code), (StatusCode::CONFLICT, "stale_version"));
        assert_eq!(e.extra.get("current"), Some(&json!(5)));
    }

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

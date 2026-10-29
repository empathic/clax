//! What an integration module asks of the instance it runs in, beyond the
//! public routes and its own user data: the instance's whole router, to
//! send public requests to in process ([`Instance::router`]), the release
//! it runs ([`Instance::release`]), its clock, and how its routes read a
//! JSON body. An outside product has each of these its own way (an HTTP
//! client to the instance's origin, its own build metadata, its own
//! framework); keeping them here leaves the integration's code what such a
//! product writes.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};

/// The instance an integration runs in.
pub struct Instance {
    state: AppState,
    router: OnceLock<axum::Router>,
}

impl Instance {
    /// The instance serving from `state`. Its router is built at the first
    /// [`Instance::router`].
    pub fn new(state: AppState) -> Arc<Instance> {
        Arc::new(Instance {
            state,
            router: OnceLock::new(),
        })
    }

    /// The instance's state: its configuration, its sessions' callers and
    /// its store, which hold the product's own user data.
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// The instance's whole router ([`crate::router::build_router`]),
    /// built once: a request sent to it meets every layer one sent over
    /// HTTP meets, with the `Host` it carries.
    pub fn router(&self) -> axum::Router {
        self.router
            .get_or_init(|| crate::router::build_router(self.state.clone()))
            .clone()
    }

    /// The release the instance runs, as `/healthz` reports it:
    /// `{version, commit?, id?}` (`id` its `build`), each a string, the
    /// last two left out when not stamped.
    pub fn release(&self) -> Value {
        let (commit, build) = crate::routes::health::stamped(&self.state);
        let mut r = json!({"version": self.state.version});
        if let Some(c) = commit {
            r["commit"] = json!(c);
        }
        if let Some(b) = build {
            r["id"] = json!(b);
        }
        r
    }

    /// Now, in Unix seconds, on the instance's clock.
    pub fn now(&self) -> i64 {
        self.state.store.now_utc().timestamp()
    }
}

/// A JSON body as the instance's routes read one: a malformed one is 400
/// `invalid_json`, one past the body limit 413 `body_too_large`.
///
/// # Errors
/// As above.
pub fn json_body<T>(r: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    crate::routes::artifacts::body(r)
}

//! `GET /_clax/containers/{id}/config.json`: a site's Clax container, which
//! the one-tag install (`<script src="{instance}/_clax/overlay.js"
//! data-container="{id}">`) and the Clax extension read before they mount
//! the overlay (`web/overlay/src/loader.ts`).
//!
//! A container is a queue's install: today its id is the queue's id. The
//! answer is `{id, version, config, overlay: {src, integrity}}`:
//! - `config` is the container's published config layer and `version` its
//!   number. No version is stored yet, so every live queue answers version
//!   `0` with an empty layer: the site's defaults and Clax's govern (the
//!   draft, publish and rollback of versions need their own table).
//! - `overlay` pins the overlay this instance serves: `src` relative to the
//!   instance, immutable at that URL, and its Subresource Integrity.
//!
//! It is credential-free and cached `no-cache` with an `ETag`. A browser on
//! one of the queue's origins may read it (`Access-Control-Allow-Origin`
//! echoes that origin); a page elsewhere may not. The extension reads it
//! from its service worker, under its host permission, so it is not bound
//! by that. A deleted or unknown queue is `404`, which the loader takes as
//! "no container".

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_http::error::ApiError;
use clax_http::http_cache::{self, REVALIDATE};
use clax_http::state::AppState;
use serde_json::{Value, json};

/// The answer for live queue `id`, pinned to `overlay`
/// (`(version, integrity)`, `None` when the UI is not built).
pub fn answer(id: &str, overlay: Option<(String, String)>) -> Value {
    let overlay = overlay.map(|(v, integrity)| {
        json!({"src": format!("/_clax/overlay.js?v={v}"), "integrity": integrity})
    });
    json!({"id": id, "version": 0, "config": {}, "overlay": overlay})
}

/// The `Access-Control-Allow-Origin` for a request from `origin`: that
/// origin when the queue lists it, else none.
pub fn allowed_origin<'a>(origin: Option<&'a str>, origins: &[String]) -> Option<&'a str> {
    let o = origin?;
    let n = clax_core::queue::normalize_origin(o).ok()?;
    origins.iter().any(|q| *q == n).then_some(o)
}

fn cors(res: &mut Response, allow: Option<&str>) {
    let h = res.headers_mut();
    h.insert(header::VARY, HeaderValue::from_static("Origin"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if let Some(v) = allow.and_then(|a| HeaderValue::from_str(a).ok()) {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
}

/// `GET /_clax/containers/{id}/config.json` (see the module documentation).
pub async fn config(
    State(s): State<AppState>,
    Path((id, file)): Path<(String, String)>,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    if file != "config.json" {
        return Err(ApiError::not_found());
    }
    let qid = id.clone();
    let row = s.store_call(move |st| st.queue(&qid)).await?;
    let origin = req.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let Some(q) = row else {
        return Err(ApiError::not_found());
    };
    let body = answer(&q.id, crate::shell::overlay_pin(&*s.web)).to_string();
    let allow = allowed_origin(origin, &q.origins);
    let mut res = http_cache::tagged(&req, body.as_bytes(), REVALIDATE, || {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            body.clone(),
        )
            .into_response()
    });
    cors(&mut res, allow);
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_listed_origin_may_read_it() {
        let origins = vec!["https://app.example.com".to_string()];
        assert_eq!(
            allowed_origin(Some("https://app.example.com"), &origins),
            Some("https://app.example.com")
        );
        assert_eq!(allowed_origin(Some("https://evil.example"), &origins), None);
        assert_eq!(allowed_origin(Some("null"), &origins), None);
        assert_eq!(allowed_origin(None, &origins), None);
    }

    #[test]
    fn the_answer_is_a_container_pinned_to_the_overlay() {
        let a = answer("q1", Some(("0123456789ab".into(), "sha256-AAAA".into())));
        assert_eq!(a["id"], "q1");
        assert_eq!(a["version"], 0);
        assert_eq!(a["config"], json!({}));
        assert_eq!(a["overlay"]["src"], "/_clax/overlay.js?v=0123456789ab");
        assert_eq!(a["overlay"]["integrity"], "sha256-AAAA");
        assert_eq!(answer("q1", None)["overlay"], Value::Null);
    }
}

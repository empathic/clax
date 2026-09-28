//! Serves the embedded web UI: the shell document and static assets.

use crate::error::ApiError;
use crate::routes::artifacts::path;
use axum::extract::Path;
use axum::extract::rejection::PathRejection;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct Assets;

pub async fn shell() -> Result<Response, ApiError> {
    match Assets::get("index.html") {
        Some(f) => Ok((
            [(header::CACHE_CONTROL, "no-store")],
            Html(f.data.into_owned()),
        )
            .into_response()),
        None => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "ui_not_built",
            "the web UI has not been built; run `just web`",
        )),
    }
}

pub async fn static_file(p: Result<Path<String>, PathRejection>) -> Result<Response, ApiError> {
    let path = path(p)?;
    let f = Assets::get(&format!("_artifax/{path}")).ok_or_else(ApiError::not_found)?;
    let ct = if path.ends_with(".js") {
        "text/javascript".to_string()
    } else {
        mime_guess::from_path(&path)
            .first_or_octet_stream()
            .to_string()
    };
    Ok((
        [
            (header::CONTENT_TYPE, ct),
            (header::CACHE_CONTROL, "public, max-age=3600".to_string()),
        ],
        f.data.into_owned(),
    )
        .into_response())
}

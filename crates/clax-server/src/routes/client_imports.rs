//! `POST /api/client-imports/{key}`: a client's one-time import of the
//! state it kept before its queue ran on the core's jobs. The store reads the
//! files as the app wrote them ([`clax_store::imports`]).

use super::artifacts::{JsonBytes, parse_body, path};
use super::jobs::require_owner;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use clax_http::agent::DeniedToGrants;
use clax_http::auth::RequireToken;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportBody {
    client: String,
    files: BTreeMap<String, String>,
}

/// `POST /api/client-imports/{key}` `{client, files}`: imports `files`
/// (each of the app's state files it holds, by name, to its text) for
/// `client`'s queue under `key` (`app-json-v1`, client `app`), in one
/// transaction with the import's record, as every job write needs the
/// queue owner's token (`x-clax-jobs-owner`; 409 `not_owner`). A key
/// imported before answers its summary with `already: true` and changes
/// nothing. 200 `{import: {already, jobs, consents, skipped, left_out,
/// done_at}}`: `skipped` names the files that did not parse, `left_out`
/// the entries naming what the core no longer holds. 400 `invalid_args`
/// for another key or client, or a file of another name.
pub async fn import(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _t: RequireToken,
    headers: HeaderMap,
    p: Result<Path<String>, PathRejection>,
    req: Result<JsonBytes, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let key = path(p)?;
    let b = parse_body(req, "the publish limit", |b: ImportBody| Ok(b)).await?;
    if b.client != "app" {
        return Err(ApiError::bad_request(
            "invalid_args",
            format!("{} keeps no state to import", b.client),
        ));
    }
    let guard = require_owner(&s, &headers, &b.client)?;
    let summary = s
        .store_call(move |st| {
            let _guard = guard;
            st.import_client_state(&key, &b.client, &b.files)
        })
        .await?;
    Ok(Json(json!({ "import": summary })))
}

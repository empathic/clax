//! `GET /api/stream`: one multiplexed event stream per client, and `POST
//! /api/stream/<id>`: its subscriptions (see [`crate::stream`]).

use super::artifacts::{body, parse_id, path};
use crate::auth::has_token;
use crate::error::ApiError;
use crate::state::AppState;
use crate::stream::{Conn, MAX_TOPICS, SubError, Topic, parse_last_event_id};
use crate::viewer::SameOrigin;
use axum::Json;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::CoreError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;

/// `GET /api/stream`: opens a stream, or resumes the one `Last-Event-ID`
/// (`<stream>:<seq>`) names when it is still held for the same caller.
///
/// The caller (the token in `Authorization` or the events cookie, and the
/// owner or viewer cookie; see [`crate::identity`]) is resolved once, here, with the levels of the `db`
/// routes; subscriptions made on the
/// stream must come from the same caller. The body opens with `event: ready`
/// (`{stream, seq, resumed, topics}`), then carries each subscribed topic's
/// events with `id: <stream>:<seq>`, `resync` (`{topic, reason}`) for a
/// topic the client must refetch, and a `: keep-alive` comment every
/// `AppState::sse_keep_alive` while idle. It ends when the daemon shuts down
/// or a newer connection resumes the same stream. A stream opened by a
/// request that may not see live pages ([`crate::live::sees_live_pages`])
/// receives no gallery event of a live page and cannot subscribe to a live
/// page's topics; it resumes only from a request of the same kind.
pub async fn open(
    State(s): State<AppState>,
    who: crate::identity::Identity,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let local = crate::live::sees_live_pages(&headers, &extensions, &s.token);
    let token = token_or_cookie(&headers, &s.token, &who);
    let caller = s
        .store_call(move |st| crate::db_caller::caller_of(st, token, &who))
        .await?;
    let resume = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_last_event_id);
    let opened = s.stream.open(caller, local, resume);
    let conn = Conn::new(
        s.stream.clone(),
        opened,
        s.sse_keep_alive,
        s.shutdown.clone(),
    );
    let chunks = futures::stream::unfold(conn, |mut c| async move {
        c.next().await.map(|b| (Ok::<_, Infallible>(b), c))
    });
    let mut res = Body::from_stream(chunks).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(res)
}

/// Whether the request holds the token: in `Authorization`, or as the events
/// cookie the shell's token request sets (scoped to `/api/events` and
/// `/api/stream`), so the browser's stream never carries the token in a URL.
/// Cookies ignore ports, so a page on another port of this host sends the
/// cookie too: it counts only on a request from this machine that the
/// browser does not mark as made from another origin ([`Identity::of`]).
///
/// [`Identity::of`]: crate::identity::Identity::of
fn token_or_cookie(headers: &HeaderMap, token: &str, who: &crate::identity::Identity) -> bool {
    has_token(headers, token) || who.events_cookie
}

/// Debug builds only: how many `/api/stream` streams the hub holds
/// (`held`, detached ones included), how many have a connection open
/// (`open`), and the caller level of each open one (`levels`), so browser
/// tests can check that a browser holds one, at the level its cookie gives.
/// `{open, held, levels}`.
#[cfg(debug_assertions)]
pub async fn open_streams(
    State(s): State<AppState>,
    _t: crate::auth::RequireToken,
) -> axum::Json<Value> {
    let st = s.stream.stats();
    let levels: Vec<Value> = s
        .stream
        .attached_levels()
        .into_iter()
        .map(|l| serde_json::to_value(l).unwrap_or(Value::Null))
        .collect();
    axum::Json(json!({ "open": st.attached, "held": st.streams, "levels": levels }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateBody {
    #[serde(default)]
    subscribe: Vec<String>,
    #[serde(default)]
    unsubscribe: Vec<String>,
}

fn topics(names: &[String]) -> Result<Vec<Topic>, ApiError> {
    names
        .iter()
        .map(|n| Topic::parse(n).map_err(|m| ApiError::bad_request("invalid_topic", m)))
        .collect()
}

/// `POST /api/stream/<id>` with `{"subscribe": [topic...], "unsubscribe":
/// [topic...]}`: changes the stream's topics in one step and answers
/// `{seq, topics}`. Each subscribed topic's events numbered above `seq`
/// reach the stream; `topics` is the stream's whole list after the change.
///
/// Checked once, here: every subscribed artifact exists, and is not a live
/// page the stream may not see (404 otherwise), and
/// a `docs` topic needs the artifact to declare `db` unless the caller holds
/// the token, in `Authorization` or as the events cookie (403 `not_declared`). 404 `unknown_stream` when no stream has
/// that ID for this caller; 429 `limit_reached` past
/// [`crate::stream::MAX_TOPICS`] topics; 400 `invalid_topic` for a name
/// that is not a topic. Refuses a foreign `Origin` like the viewer routes.
pub async fn update(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: crate::identity::Identity,
    sees: crate::live::SeesLive,
    p: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
    req: Result<Json<UpdateBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = path(p)?;
    let b = body(req)?;
    if b.subscribe.len() > MAX_TOPICS || b.unsubscribe.len() > MAX_TOPICS {
        return Err(ApiError::bad_request(
            "invalid_topic",
            format!("at most {MAX_TOPICS} topics a request"),
        ));
    }
    let add = topics(&b.subscribe)?;
    let remove = topics(&b.unsubscribe)?;
    for aid in add.iter().filter_map(Topic::artifact) {
        sees.check(&s.live_ids, aid)?;
    }
    let token = token_or_cookie(&headers, &s.token, &who);
    let check = add.clone();
    let caller = s
        .store_call(move |st| {
            for t in &check {
                let Some(aid) = t.artifact() else { continue };
                let aid = parse_id(aid).map_err(|_| CoreError::NotFound)?;
                st.get_artifact(&aid)?.ok_or(CoreError::NotFound)?;
                if matches!(t, Topic::Docs(_)) && !token && !st.doc_declared(&aid)? {
                    return Err(CoreError::NotDeclared { capability: "db" });
                }
            }
            crate::db_caller::caller_of(st, token, &who)
        })
        .await?;
    match s.stream.update(&id, &caller, &add, &remove) {
        Ok((seq, topics)) => Ok(Json(json!({"seq": seq, "topics": topics}))),
        Err(SubError::UnknownStream) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "unknown_stream",
            "no open stream has that ID for this caller; open /api/stream again",
        )),
        Err(SubError::Hidden) => Err(CoreError::NotFound.into()),
        Err(SubError::TooMany) => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "limit_reached",
            format!("a stream holds at most {MAX_TOPICS} topics"),
        )),
    }
}

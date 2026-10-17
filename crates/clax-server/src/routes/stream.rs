//! `GET /api/stream`: one multiplexed event stream per client, and `POST
//! /api/stream/<id>`: its subscriptions (see [`clax_realtime::stream`]).

use super::artifacts::{body, path};
use axum::Json;
#[cfg(not(target_arch = "wasm32"))]
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
#[cfg(not(target_arch = "wasm32"))]
use axum::http::HeaderValue;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::CoreError;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::seats::Subscribe;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_realtime::stream::{Conn, MAX_TOPICS, SubError, Topic, parse_last_event_id};
use serde::Deserialize;
use serde_json::{Value, json};
#[cfg(not(target_arch = "wasm32"))]
use std::convert::Infallible;

/// `GET /api/stream`: opens a stream, or resumes the one `Last-Event-ID`
/// (`<stream>:<seq>`) names when it is still held for the same caller.
///
/// The caller (the token in `Authorization` or the events cookie, and the
/// owner or viewer cookie; see [`clax_http::identity`]) is resolved once, here, with the levels of the `db`
/// routes; subscriptions made on the
/// stream must come from the same caller. The body opens with `event: ready`
/// (`{stream, seq, resumed, topics, caller}`, `caller` being `{level,
/// viewer}`: the level and viewer public ID the stream was opened for), then carries each subscribed topic's
/// events with `id: <stream>:<seq>`, `resync` (`{topic, reason}`) for a
/// topic the client must refetch, and a `: keep-alive` comment every
/// `AppState::sse_keep_alive` while idle. It ends when the daemon shuts down
/// or a newer connection resumes the same stream. A stream opened by a
/// request that may not see live pages ([`clax_http::live_access::sees_live_pages`])
/// receives no gallery event of a live page and cannot subscribe to a live
/// page's topics; it resumes only from a request of the same kind. A stream
/// is opened for the person the request speaks for (the owner, or a
/// connected person's browser), whose `questions` and `inbox` topics alone
/// it may hold; a person's stream, and a hosted daemon's guest's, carries
/// agents' drafts on its `working` topics; a person's stream ends once the
/// person is revoked. A stream
/// opened through the extension gateway is live-only
/// ([`clax_realtime::stream::live_only_admits`]); only requests with the credential
/// it was opened with change or resume it, and it ends, delivering nothing
/// more, once that credential is revoked or expires. On a hosted daemon a
/// client holds at most [`clax_http::socket_caps`]'s caps of streams and
/// sockets: past one, 429 `too_many_streams` (a resume of a stream whose
/// connection is still open takes its place).
#[cfg(not(target_arch = "wasm32"))]
pub async fn open(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    who: clax_http::identity::Identity,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // The stream serves people (spec §7.5): an agent reads through the
    // filtered routes.
    d.refuse_agents()?;
    let local = clax_http::live_access::sees_live_pages(&headers, &extensions, &s.token);
    let credential = extensions
        .get::<clax_http::extension::ExtensionCredential>()
        .map(|c| c.hash().to_string());
    let resume = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_last_event_id);
    let guest = hosted_guest(&extensions);
    let client = clax_http::socket_caps::SocketClient::of_request(&s, &headers, &extensions);
    let slot = slot_for(&s, &client, resume)?;
    let conn = open_stream(&s, who, local, credential, resume, guest)
        .await?
        .holding(Some(slot));
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

/// Opens (or, with `resume`, reattaches) the stream of the request whose
/// credentials are `who`, as [`open`] does for every outlet: the caller and
/// person resolved once, from the store, and a stream opened through the
/// extension gateway (`credential`, the hash of its credential) ending once
/// that credential stops being live. `local`: whether the stream may see
/// live pages ([`clax_http::live_access::sees_live_pages`]). `guest`: whether the
/// request is a hosted daemon's signed-in guest, whose stream carries
/// agents' drafts as a person's does ([`hosted_guest`]).
///
/// # Errors
/// The store's.
pub async fn open_stream(
    s: &AppState,
    who: clax_http::identity::Identity,
    local: bool,
    credential: Option<String>,
    resume: Option<(&str, u64)>,
    guest: bool,
) -> Result<Conn, ApiError> {
    let token = token_or_cookie(&who);
    let (caller, person) = s
        .store_call(move |st| {
            let caller = clax_http::db_caller::caller_of(st, token, &who)?;
            Ok((caller, person_key(st, &who)))
        })
        .await?;
    let drafts = person.is_some() || guest;
    let opened = s
        .stream
        .open_with(caller, local, credential.clone(), resume, person, drafts);
    let mut conn = Conn::new(
        s.stream.clone(),
        opened,
        s.sse_keep_alive,
        s.shutdown.clone(),
    );
    if let Some(hash) = credential {
        let (creds, id) = (s.ext_creds.clone(), s.extension_id.clone());
        conn = conn.checking(move || clax_http::extension::is_live(&creds, &hash, &id));
    }
    Ok(conn)
}

/// Whether a WebSocket upgrade of `/api/stream` may open: browsers send
/// `Origin` on every WebSocket handshake and apply no CORS to it, so a
/// page of any other origin (an artifact's `<id>.localhost`, or another
/// site the viewer's cookie is sent from) could otherwise read the viewer's
/// stream. Admitted: no `Origin` (a script, curl) unless `Sec-Fetch-Site`
/// names another origin, or an `Origin` of `http://` or `https://` and the
/// request's own `Host`, when that host is not an artifact's.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn socket_origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return !clax_http::viewer::fetched_from_elsewhere(headers);
    };
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    match (origin.to_str().ok(), host) {
        (Some(o), Some(h)) if !h.is_empty() && clax_http::host::artifact_host(h).is_none() => o
            .strip_prefix("http://")
            .or_else(|| o.strip_prefix("https://"))
            .is_some_and(|rest| rest.eq_ignore_ascii_case(h)),
        _ => false,
    }
}

/// A stream opened for a socket: the response of `GET /api/stream` on
/// wasm32 carries it as an extension, and the host that upgrades the
/// request to its own socket takes it (the stream's connection and the
/// credentials to reopen it with).
#[derive(Clone)]
pub struct SocketStream(
    pub std::sync::Arc<std::sync::Mutex<Option<(Conn, clax_http::identity::SocketCredentials)>>>,
);

/// `GET /api/stream` on wasm32, where the stream rides a WebSocket the
/// host (a Durable Object) holds, not a response body. A WebSocket upgrade
/// request is checked and its stream opened as [`open`] does (resuming the
/// one its `last_event_id` query parameter names, `<stream>:<seq>`, as
/// `Last-Event-ID` does), and answered `200` with an empty body and a
/// [`SocketStream`] extension: the host replaces the response with its
/// socket, which carries the stream's frames, one SSE block a text
/// message. Any other request is 426 `websocket_required`, which tells the
/// shell to use the WebSocket.
#[cfg(target_arch = "wasm32")]
pub async fn open_socket(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    who: clax_http::identity::Identity,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let upgrade = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if !upgrade {
        return Err(ApiError::new(
            StatusCode::UPGRADE_REQUIRED,
            "websocket_required",
            "this server carries /api/stream over a WebSocket",
        ));
    }
    if !socket_origin_allowed(&headers) {
        return Err(ApiError::forbidden(
            "forbidden_origin",
            "the stream's WebSocket opens only from the Clax shell's own origin",
        ));
    }
    let local = clax_http::live_access::sees_live_pages(&headers, &extensions, &s.token);
    let credential = extensions
        .get::<clax_http::extension::ExtensionCredential>()
        .map(|c| c.hash().to_string());
    let last = query.as_deref().and_then(|q| {
        url::form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == "last_event_id")
            .map(|(_, v)| v.into_owned())
    });
    let resume = last.as_deref().and_then(parse_last_event_id);
    let guest = hosted_guest(&extensions);
    let client = clax_http::socket_caps::SocketClient::of_request(&s, &headers, &extensions);
    let slot = slot_for(&s, &client, resume)?;
    let mut creds = clax_http::identity::SocketCredentials::of_request(&who, &s.token, &extensions);
    creds.address = client.address();
    let conn = open_stream(&s, who, local, credential, resume, guest)
        .await?
        .holding(Some(slot));
    let mut res = StatusCode::OK.into_response();
    res.extensions_mut()
        .insert(SocketStream(std::sync::Arc::new(std::sync::Mutex::new(
            Some((conn, creds)),
        ))));
    Ok(res)
}

/// The place under `client`'s cap a stream takes ([`clax_http::socket_caps`]):
/// a resume of a stream whose connection is still open takes that one's,
/// which ends.
///
/// # Errors
/// 429 `too_many_streams`.
fn slot_for(
    s: &AppState,
    client: &clax_http::socket_caps::SocketClient,
    resume: Option<(&str, u64)>,
) -> Result<clax_http::socket_caps::SocketSlot, ApiError> {
    if resume.is_some_and(|(id, _)| s.stream.is_attached(id)) {
        return Ok(s.sockets.hold(client));
    }
    s.sockets.admit(client)
}

/// Whether the request is a hosted daemon's signed-in guest: their stream
/// carries agents' drafts on the pages the gate lets them follow, as a
/// person's does.
fn hosted_guest(extensions: &axum::http::Extensions) -> bool {
    extensions
        .get::<clax_http::hosted::HostedRequest>()
        .is_some_and(|h| h.session.as_ref().is_some_and(|v| v.guest))
}

/// The recipient key of the person `who` speaks for, whose `questions` and
/// `inbox` topics a stream may hold: empty for the owner, a connected
/// person's public ID for their browser, `None` for anyone else.
fn person_key(st: &clax_store::Store, who: &clax_http::identity::Identity) -> Option<String> {
    match who.person(st) {
        Ok(clax_http::auth::Person::Owner) => Some(String::new()),
        Ok(clax_http::auth::Person::Viewer(pid)) => Some(pid),
        _ => None,
    }
}

/// Whether the request holds the token: in `Authorization`, or as the events
/// cookie the shell's token request sets (scoped to `/api/events` and
/// `/api/stream`), so the browser's stream never carries the token in a URL.
/// Cookies ignore ports, so a page on another port of this host sends the
/// cookie too: it counts only on a request from this machine that the
/// browser does not mark as made from another origin ([`Identity::of`]).
///
/// [`Identity::of`]: clax_http::identity::Identity::of
fn token_or_cookie(who: &clax_http::identity::Identity) -> bool {
    who.token || who.events_cookie
}

/// Debug builds only: how many `/api/stream` streams the hub holds
/// (`held`, detached ones included), how many have a connection open
/// (`open`), the caller level of each open one (`levels`), how many it
/// has opened since it started (`opened`; a resume opens none), and whether
/// an owner surface is open for mirrored questions (`owner_surface`), so
/// browser tests can check that a browser holds one, at the level its
/// cookie gives, count the streams it opens, and wait for a surface.
/// `{open, held, levels, opened, owner_surface}`.
#[cfg(debug_assertions)]
pub async fn open_streams(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: clax_http::auth::RequireToken,
) -> axum::Json<Value> {
    let st = s.stream.stats();
    let levels: Vec<Value> = s
        .stream
        .attached_levels()
        .into_iter()
        .map(|l| serde_json::to_value(l).unwrap_or(Value::Null))
        .collect();
    axum::Json(
        json!({ "open": st.attached, "held": st.streams, "levels": levels, "opened": st.opened, "owner_surface": s.stream.holds_owner_topics() }),
    )
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
/// page the stream may not see (404 otherwise), a `site:<origin>` topic
/// needs a stream that may see live pages (404 otherwise) and an owner
/// credential (403 `forbidden` otherwise), `questions` and `inbox` need the
/// owner or a connected person, the person the stream was opened for (403
/// `forbidden` otherwise, and the whole change is refused),
/// and a `docs` topic needs the artifact to declare `db` unless the caller holds
/// the token, in `Authorization` or as the events cookie (403 `not_declared`). 404 `unknown_stream` when no stream has
/// that ID for this caller (a stream opened through the extension gateway
/// is changed only with its own credential, and any other only without
/// one); 429 `limit_reached` past
/// [`clax_realtime::stream::MAX_TOPICS`] topics; 429 `no_live_seat` when an
/// artifact's `artifact`, `presence` or `working` topic is asked for and no
/// live seat is free for the caller there ([`clax_http::seats`]: the stream
/// holds its person's seat while it holds any of them); 400 `invalid_topic` for a name
/// that is not a topic. A hosted daemon's person whose rights do not read
/// an artifact's comments ([`clax_http::hosted::reads_comments`]) holds its
/// `artifact` topic without thread events ([`Topic::ArtifactQuiet`]). On a
/// live-only stream (the extension's), 403
/// `forbidden` for `gallery`, a `docs` topic, or a topic of an artifact
/// that is not a live page (`questions` and `inbox` it may take). Refuses a foreign `Origin` like the viewer routes.
#[allow(clippy::too_many_arguments)]
pub async fn update(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    _o: SameOrigin,
    who: clax_http::identity::Identity,
    sees: clax_http::live_access::SeesLive,
    via: Option<axum::Extension<clax_http::extension::ExtensionCredential>>,
    hosted: Option<axum::Extension<clax_http::hosted::HostedRequest>>,
    p: Result<Path<String>, PathRejection>,
    req: Result<Json<UpdateBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let id = path(p)?;
    let b = body(req)?;
    if b.subscribe.len() > MAX_TOPICS || b.unsubscribe.len() > MAX_TOPICS {
        return Err(ApiError::bad_request(
            "invalid_topic",
            format!("at most {MAX_TOPICS} topics a request"),
        ));
    }
    let add = topics(&b.subscribe)?;
    let mut remove = topics(&b.unsubscribe)?;
    // An `artifact` topic is held loud or quiet: leaving it leaves either.
    let quiet: Vec<Topic> = remove
        .iter()
        .filter(|t| matches!(t, Topic::Artifact(_)))
        .map(|t| t.clone().without_comments())
        .collect();
    remove.extend(quiet);
    let member = hosted.and_then(|h| h.0.member);
    if sees.live_only
        && !add
            .iter()
            .all(|t| clax_realtime::stream::live_only_admits(&s.live_ids, t))
    {
        return Err(live_only_refusal());
    }
    for aid in add.iter().filter_map(Topic::artifact) {
        sees.check(&s.live_ids, aid)?;
    }
    if add.iter().any(|t| matches!(t, Topic::Site(_))) {
        if !sees.may_see {
            return Err(CoreError::NotFound.into());
        }
        // The read rule of `GET /api/live/site`.
        if !who.is_owner() {
            return Err(ApiError::forbidden(
                "forbidden",
                "only the owner follows a site's threads",
            ));
        }
    }
    if add.contains(&Topic::Questions)
        || add.contains(&Topic::Inbox)
        || add.contains(&Topic::Agents)
    {
        let w = who.clone();
        if s.store_call(move |st| Ok(person_key(st, &w)))
            .await?
            .is_none()
        {
            return Err(ApiError::forbidden(
                "forbidden",
                "only the owner or a connected person follows questions, an inbox and the agents",
            ));
        }
    }
    if add.contains(&Topic::People) && !who.is_owner() {
        return Err(ApiError::forbidden(
            "forbidden",
            "only the owner follows people, invites and grants",
        ));
    }
    let token = token_or_cookie(&who);
    let check = add.clone();
    let (caller, claims, add) = s
        .store_call(move |st| {
            let mut live = std::collections::BTreeSet::new();
            for t in &check {
                let Some(aid) = t.artifact() else { continue };
                let aid = parse_id(aid).map_err(|_| CoreError::NotFound)?;
                st.get_artifact(&aid)?.ok_or(CoreError::NotFound)?;
                if matches!(t, Topic::Docs(_)) && !token && !st.doc_declared(&aid)? {
                    return Err(CoreError::NotDeclared { capability: "db" });
                }
                if t.is_live() {
                    live.insert(aid.as_str().to_string());
                }
            }
            // The live seat each artifact's live topics take (spec
            // 2026-10-11-clax-dev-hosted-design §16.1).
            let mut claims = Vec::new();
            if !live.is_empty() {
                let person = who.caller(st, None).map_err(|_| CoreError::NotFound)?;
                for aid in live {
                    if let Some(c) = clax_http::seats::claim_for(st, &person, &aid)? {
                        claims.push((aid, c));
                    }
                }
            }
            // Each `artifact` topic is held quiet where its comments are not
            // the caller's to read.
            let mut add = Vec::with_capacity(check.len());
            for t in check {
                let quiet = matches!(&t, Topic::Artifact(a)
                    if !clax_http::hosted::reads_comments(st, member.as_ref(), a)?);
                add.push(if quiet { t.without_comments() } else { t });
            }
            Ok((
                clax_http::db_caller::caller_of(st, token, &who)?,
                claims,
                add,
            ))
        })
        .await?;
    let credential = via.as_ref().map(|c| c.0.hash());
    let result = clax_http::seats::subscribe(&s, &id, &caller, credential, &claims, &add, &remove);
    match result {
        Ok((seq, topics)) => Ok(Json(json!({"seq": seq, "topics": topics}))),
        Err(Subscribe::NoSeat) => Err(no_seat()),
        Err(Subscribe::Hub(SubError::UnknownStream)) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "unknown_stream",
            "no open stream has that ID for this caller; open /api/stream again",
        )),
        Err(Subscribe::Hub(SubError::Hidden)) => Err(CoreError::NotFound.into()),
        Err(Subscribe::Hub(SubError::Forbidden)) => Err(live_only_refusal()),
        Err(Subscribe::Hub(SubError::TooMany)) => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "limit_reached",
            format!("a stream holds at most {MAX_TOPICS} topics"),
        )),
    }
}

/// 429 `no_live_seat`: no live seat is free for the caller on an artifact
/// whose live topics it asked for (spec 2026-10-11-clax-dev-hosted-design
/// §16.1); its page turns spectator and polls the snapshot.
fn no_seat() -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        clax_core::seats::NO_SEAT_CODE,
        clax_core::seats::NO_SEAT_MESSAGE,
    )
}

/// 403 `forbidden`: a topic a live-only stream may not take.
fn live_only_refusal() -> ApiError {
    ApiError::forbidden(
        "forbidden",
        "the extension's stream takes live pages' artifact, presence and working topics only",
    )
}

#[cfg(test)]
mod tests {
    use super::socket_origin_allowed;
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    fn h(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        m
    }

    #[test]
    fn the_streams_websocket_opens_only_from_the_shells_own_origin() {
        let host = ("host", "localhost:8787");
        for ok in [
            h(&[host]),
            h(&[host, ("origin", "http://localhost:8787")]),
            h(&[("host", "clax.example"), ("origin", "https://clax.example")]),
        ] {
            assert!(socket_origin_allowed(&ok), "{ok:?}");
        }
        for refused in [
            h(&[host, ("origin", "http://7q3k9mzx2b4t.localhost:8787")]),
            h(&[
                ("host", "7q3k9mzx2b4t.localhost:8787"),
                ("origin", "http://7q3k9mzx2b4t.localhost:8787"),
            ]),
            h(&[host, ("origin", "https://evil.example")]),
            h(&[host, ("origin", "http://localhost:8788")]),
            h(&[host, ("origin", "null")]),
            h(&[("origin", "http://localhost:8787")]),
            h(&[host, ("sec-fetch-site", "cross-site")]),
        ] {
            assert!(!socket_origin_allowed(&refused), "{refused:?}");
        }
    }
}

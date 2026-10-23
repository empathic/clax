//! Agents' drafts (spec 2026-10-11-clax-dev-hosted-design §16.4): an agent
//! session sends its work in progress on an artifact, people list the live
//! drafts and show one in the frame from its draft URL.
//!
//! Rights map onto the agent model: drafting needs what publishing that
//! artifact needs (`clax-artifacts` `update`, from a session), and a draft
//! reaches named people only: the owner and connected people, who read
//! every artifact the daemon serves them, and on a hosted daemon the
//! members who may read the artifact and the guests of a page shared by
//! link (the gate, [`clax_http::hosted`], answers 404 or 401 to anyone else
//! first). The listing answers them alone, the stream sends `drafts` events
//! to their streams alone, and the draft URL refuses agents; its token
//! reaches no one else, so it is the draft's capability, on a hosted daemon
//! as a version's content key is.

use super::artifacts::{body_within, path};
use axum::Json;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::ArtifactId;
use clax_core::CoreError;
use clax_core::drafts::{
    DraftFile, Drafter, MAX_DRAFT_BYTES, MAX_NOTE_CHARS, NewDraft, Refusal, Served, is_token,
    served_page,
};
use clax_core::model::FileMeta;
use clax_core::publish::{FileChange, FileInput, check_path, content_type_for};
use clax_core::working::Actor;
use clax_http::agent::{DeniedToGrants, TokenOrAgent};
use clax_http::audit::DeferredAudit;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::state::AppState;
use clax_http::views::thread::VisFor;
use clax_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The most bytes of base files one revision's pages are compared with: a
/// base page past what is left of it is compared with nothing (every
/// element with text is marked), so a revision costs at most this much
/// reading and marking whatever the base version holds.
pub const MAX_BASE_COMPARED: u64 = 2 * MAX_DRAFT_BYTES as u64;

pub use clax_http::limits::DRAFT_BODY_LIMIT;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBody {
    /// Path to the file's draft content (the publish shape); `null` removes
    /// the path.
    files: BTreeMap<String, Option<FileInput>>,
    #[serde(default)]
    note: Option<String>,
    /// The version the draft is based on: the artifact's current one.
    #[serde(default)]
    if_version: Option<u32>,
}

/// The live session `sid`: 404 when unknown, 400 `unknown_session` when ended.
fn live_session(st: &Store, sid: &str) -> clax_core::Result<clax_core::model::Session> {
    match st.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(s) if s.ended_at.is_some() => Err(CoreError::invalid(
            "unknown_session",
            "the session has ended",
        )),
        Some(s) => Ok(s),
    }
}

/// The bytes and metadata of `path` in version `n`, read from the content
/// store; `None` when the version records no such file or its bytes are
/// gone. The one place drafts read a version's files, awaiting the
/// content store ([`Store::read_file_async`]).
async fn base_file(
    st: &Store,
    id: &ArtifactId,
    n: u32,
    path: &str,
) -> clax_core::Result<Option<(Vec<u8>, FileMeta)>> {
    st.read_file_async(id, n, path).await
}

/// The text of base page `path` of version `n` to compare a draft page
/// with, while `budget` (bytes, [`MAX_BASE_COMPARED`] a revision) holds
/// its size: `None` for a missing or non-UTF-8 file, or one past it.
async fn base_text(
    st: &Store,
    id: &ArtifactId,
    n: u32,
    path: &str,
    budget: &mut u64,
) -> clax_core::Result<Option<String>> {
    match st.file_meta(id, n, path)? {
        Some(meta) if meta.size <= *budget => *budget -= meta.size,
        _ => return Ok(None),
    }
    Ok(base_file(st, id, n, path)
        .await?
        .and_then(|(b, _)| String::from_utf8(b).ok()))
}

/// Whether a content type is `text/html` (parameters ignored).
fn is_html(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("text/html")
}

/// The draft's files decoded: each path safe (one the artifact's site
/// serves itself is refused where its row is read), at most [`MAX_DRAFT_BYTES`]
/// in all (413 `too_large`), at least one (400 `invalid_args`).
fn decode(
    files: BTreeMap<String, Option<FileInput>>,
) -> Result<BTreeMap<String, FileChange>, ApiError> {
    use base64::Engine as _;
    if files.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_args",
            "a draft carries at least one file; DELETE the draft to end it",
        ));
    }
    let mut total = 0usize;
    let mut out = BTreeMap::new();
    for (p, f) in files {
        check_path(&p)?;
        let change = match f {
            None => FileChange::Remove,
            Some(f) => {
                let bytes = match f.encoding {
                    clax_core::publish::Encoding::Utf8 => f.content.into_bytes(),
                    clax_core::publish::Encoding::Base64 => {
                        base64::engine::general_purpose::STANDARD
                            .decode(
                                f.content
                                    .bytes()
                                    .filter(|b| !b.is_ascii_whitespace())
                                    .collect::<Vec<u8>>(),
                            )
                            .map_err(|_| {
                                ApiError::bad_request(
                                    "invalid_encoding",
                                    format!("'{p}' is not valid base64"),
                                )
                            })?
                    }
                };
                total += bytes.len();
                if total > MAX_DRAFT_BYTES {
                    return Err(ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "too_large",
                        format!("a draft may carry at most {MAX_DRAFT_BYTES} bytes of files"),
                    ));
                }
                let content_type = f.content_type.unwrap_or_else(|| content_type_for(&p));
                FileChange::Put(clax_core::publish::DecodedFile {
                    bytes,
                    content_type,
                })
            }
        };
        out.insert(p, change);
    }
    Ok(out)
}

fn refused(r: Refusal) -> ApiError {
    match r {
        Refusal::RateLimited { retry_after_ms } => {
            let mut e = ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                format!(
                    "at most {} drafts a second: wait {retry_after_ms} ms and send your latest draft",
                    clax_core::drafts::DRAFT_RATE
                ),
            );
            e.extra
                .insert("retry_after_ms".into(), json!(retry_after_ms));
            e
        }
        Refusal::TooManyDrafts => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "limit_reached",
            format!(
                "at most {} drafts are live at once; try again later",
                clax_core::drafts::MAX_LIVE_DRAFTS
            ),
        ),
        Refusal::TooManyOfSession => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "limit_reached",
            format!(
                "a session keeps at most {} drafts live at once: publish or drop one (`done: true`) first",
                clax_core::drafts::MAX_SESSION_DRAFTS
            ),
        ),
    }
}

/// `PUT /api/sessions/<sid>/drafts/<aid>`: the session's draft on the
/// artifact, replacing any before it: `files` in the publish shape (`null`
/// removes a path), at most [`MAX_DRAFT_BYTES`] in all, over the current
/// version (`if_version`, when given, must be it: 409 `conflict`), with an
/// optional `note`. Needs what publishing needs (`clax-artifacts`
/// `update`), and at most [`clax_core::drafts::DRAFT_RATE`] calls a second
/// (429 `rate_limited` with `retry_after_ms`). A drafted HTML page is
/// marked against the base file at its path only when the drafting agent
/// may see that file (and within [`MAX_BASE_COMPARED`]); a base file it may
/// not see is never served through the draft. Named people's streams get a `drafts` event. Drafting marks
/// the session working on the artifact, as feedback reaching it does
/// (`working.start` for a new record). `{draft, note_truncated}`; the
/// draft's token is not in it.
pub async fn put(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::DraftPut>,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<PutBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, _) = path(p)?;
    let id = a.artifact()?;
    if s.live_ids.contains(id.as_str()) {
        return Err(ApiError::bad_request(
            "live_page",
            "a live page takes snapshots from the Clax extension; it has no drafts",
        ));
    }
    clax_http::agent::person_agent_session(&a.caller)?;
    let b = body_within(req, "the draft limit")?;
    let files = decode(b.files)?;
    let (note, truncated) = match b.note.as_deref() {
        Some(n) => clax_core::working::clean_line(n, MAX_NOTE_CHARS),
        None => (None, false),
    };
    let if_version = b.if_version;
    let vf = VisFor::of(&a.caller, &s);
    let (drafts, events, working) = (s.drafts.clone(), s.events.clone(), s.working.clone());
    let view = s
        .store_call_async(move |st| async move {
            let st = &*st;
            let sess = live_session(st, &sid)?;
            let audit = audit.for_session(st, &sid)?;
            let art = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let n = art.current_version;
            if if_version.is_some_and(|v| v != n) {
                return Err(CoreError::Conflict { current: n });
            }
            // A file the artifact's site serves itself is refused, as
            // publishing refuses it; one may still be removed.
            for (path, change) in &files {
                if matches!(change, FileChange::Put(_)) {
                    clax_core::publish::check_path_for(path, art.legacy_paths)?;
                }
            }
            let actor = Actor {
                session_id: sess.id.clone(),
                harness: sess.harness.clone(),
                agent: sess.agent_handle.clone(),
                name: sess.name.clone(),
            };
            let who = Drafter {
                session_id: sess.id,
                agent: sess.agent_handle,
                harness: sess.harness,
            };
            if let Err(r) = drafts.admit(&who, id.as_str()) {
                return Ok(Err(refused(r)));
            }
            let base = st.get_version(&id, n)?.ok_or(CoreError::NotFound)?;
            let withheld: BTreeSet<String> = vf
                .build(st)?
                .filter_hidden_files(&base)
                .into_iter()
                .collect();
            let mut out = BTreeMap::new();
            let mut budget = MAX_BASE_COMPARED;
            for (path, change) in files {
                let file = match change {
                    FileChange::Remove => DraftFile::Removed,
                    FileChange::Put(f) => {
                        let text = is_html(&f.content_type)
                            .then(|| std::str::from_utf8(&f.bytes).ok())
                            .flatten();
                        let page = match text {
                            Some(text) => {
                                // Compared only with a base file the drafting
                                // agent may see; the page gets marks, never base text.
                                let base_text = if base.files.contains_key(&path)
                                    && !withheld.contains(&path)
                                {
                                    base_text(st, &id, n, &path, &mut budget).await?
                                } else {
                                    None
                                };
                                Some(served_page(text, base_text.as_deref()))
                            }
                            None => None,
                        };
                        DraftFile::Bytes {
                            bytes: page.map_or(f.bytes, String::into_bytes).into(),
                            content_type: f.content_type,
                        }
                    }
                };
                out.insert(path, file);
            }
            let new = NewDraft {
                base_version: n,
                note,
                files: out,
                withheld,
            };
            let (view, changed) = match drafts.put(&who, id.as_str(), new) {
                Ok(v) => v,
                Err(r) => return Ok(Err(refused(r))),
            };
            clax_realtime::drafts::announce(&events, &drafts, &changed);
            // The agent is at work on the artifact while it drafts.
            let marked = working.mark(&actor, id.as_str(), &[]);
            clax_realtime::working::settle(st, &audit, &events, &working, &marked);
            Ok(Ok(view))
        })
        .await??;
    Ok(Json(json!({
        "draft": {
            "artifact_id": a.artifact()?.as_str(),
            "base_version": view.base_version,
            "paths": view.paths,
            "note": view.note,
            "updated_at": view.updated_at,
        },
        "note_truncated": truncated,
    })))
}

/// `DELETE /api/sessions/<sid>/drafts/<aid>`: ends the session's draft on
/// the artifact, telling people. `{ended}`: whether there was one.
pub async fn delete(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::DraftDelete>,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, _) = path(p)?;
    let id = a.artifact()?;
    let (drafts, events) = (s.drafts.clone(), s.events.clone());
    let ended = s
        .store_call(move |st| {
            live_session(st, &sid)?;
            Ok(clax_realtime::drafts::end(
                &events,
                &drafts,
                &sid,
                id.as_str(),
            ))
        })
        .await?;
    Ok(Json(json!({"ended": ended})))
}

/// `GET /api/artifacts/<aid>/drafts` (named people): `{drafts}`, the live
/// drafts, oldest revision first, each with its revision's token for the
/// draft URL. Answers the owner, connected people and a hosted daemon's
/// guests only: 403 `not_for_agents` to the owner's agents, 403
/// `forbidden` to a person's agent, a LAN viewer who is not connected and
/// no credential.
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    hosted: Option<Extension<clax_http::hosted::HostedRequest>>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let guest = hosted.is_some_and(|h| h.session.as_ref().is_some_and(|v| v.guest));
    match d.caller(&s).await? {
        Caller::Owner { .. } | Caller::Person { .. } => {}
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => {
            return Err(clax_http::auth::not_for_agents());
        }
        // The gate let the guest through on the page's link.
        Caller::Viewer { .. } if guest => {}
        Caller::Viewer { .. } | Caller::Anonymous => {
            return Err(ApiError::forbidden(
                "forbidden",
                "only the owner or a connected person sees agents' drafts",
            ));
        }
    }
    let id = parse_id(&path(p)?)?;
    let drafts = s.drafts.clone();
    let list = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            Ok(drafts.for_artifact(id.as_str()))
        })
        .await?;
    Ok(Json(json!({"drafts": list})))
}

/// File `rel` of agent `agent`'s draft revision `token` on `aid`: the
/// draft's bytes for a path it writes (its HTML marked and tinted), or the
/// base version's for one it does not, as stored (never wrapped: a draft
/// page does not greet the shell). Served like content (the content
/// policy for its origin), never cached (`no-store`), and with
/// `Referrer-Policy: no-referrer`, so the page's requests do not carry its
/// token in `Referer` (a page's own `<meta name="referrer">` still
/// overrides it). 404 for an
/// unknown, revised or ended draft, a path it removes, and a base file
/// withheld from the drafting agent.
pub(crate) async fn serve(
    s: &AppState,
    aid: &str,
    agent: &str,
    token: &str,
    rel: String,
    req: &HeaderMap,
) -> Result<Response, ApiError> {
    let id = parse_id(aid)?;
    if !is_token(token) {
        return Err(ApiError::not_found());
    }
    let served = s
        .drafts
        .file(id.as_str(), agent, token, &rel)
        .ok_or_else(ApiError::not_found)?;
    let (body, content_type) = match served {
        Served::Draft {
            bytes,
            content_type,
        } => (Body::from(bytes.to_vec()), content_type),
        Served::Base { n } => {
            let (bytes, meta) = s
                .store_call_async(move |st| async move {
                    base_file(&st, &id, n, &rel)
                        .await?
                        .ok_or(CoreError::NotFound)
                })
                .await?;
            (Body::from(bytes), meta.content_type)
        }
    };
    let mut res = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
        ],
        body,
    )
        .into_response();
    res.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        super::content::content_policy(s, req),
    );
    res.extensions_mut()
        .insert(clax_http::content_policy::ContentPolicySet);
    Ok(res)
}

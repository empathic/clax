//! The decisions that matter, confirmed in the owner's browser while an
//! agent is live.
//!
//! An agent running as the owner's OS user can read the daemon token
//! (`<home>/daemon.json`) and call as the owner by hand. So while an agent
//! of the owner's on this machine is live, or ended less than
//! [`CONFIRMATION_TTL_SECS`] ago (so ending every session with the token
//! does not turn this off; [`clax_store::Store::agent_session_live`]), a
//! short list of decisions ([`Decision`]) made without a browser the owner
//! trusts waits for a click in that browser before it takes effect:
//!
//! 1. answering a question an agent asked;
//! 2. pushing an artifact to a remote;
//! 3. connecting a remote;
//! 4. widening who may open a page (its site to the world, its public
//!    copy's link access);
//! 5. approving a machine's connection, setting a person's rights, and
//!    revoking a person;
//! 6. pairing the Clax extension (minting its credential), which then
//!    acts as the owner's browser.
//!
//! An agent of another machine (a session with a grant) never holds the
//! token and makes nothing wait. A session an open job names is never
//! reaped, so it counts for as long as the job is open: its harness may
//! still be running.
//!
//! A route calls [`require_confirmation`] first, before it changes
//! anything. For a caller that needs one, its first call answers 202
//! `{status: "confirmation_pending", confirmation_id, confirm_url,
//! decision}` and changes nothing; the owner confirms or declines it in
//! their browser (`POST /api/confirmations/<id>`), and the same call made
//! again with the confirmation's ID in [`CONFIRMATION_HEADER`] goes ahead
//! once it is approved: 403 `declined` once declined, 410
//! `confirmation_expired` after [`CONFIRMATION_TTL_SECS`]. A confirmation
//! lets its own decision through once, and no other.
//!
//! Who needs one ([`Confirming`]): the owner (`Caller::Owner`) calling with
//! the token and no browser credential, with the app's key cookie (its
//! value is the token), or from a browser session whose login code was
//! minted while an agent session was live
//! ([`crate::local_trust::OwnerSessions::minted_while_agent`]): an agent
//! holding the token could have opened any of them. An owner browser
//! session opened otherwise, the in-process app's frame (the Clax app's
//! shell, relayed through `clax_embed_call`) and a paired extension need
//! none, nor does any caller while no agent session is live. Every
//! other caller (the owner's agents, people and their agents) is left to
//! the route, as before: this adds no power to anyone. A hosted daemon,
//! whose token no local agent reads, asks for none.

use crate::auth::Caller;
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use clax_store::Store;
use clax_store::confirmations::{Confirmation, Use};
use serde::Serialize;
use serde_json::{Value, json};

pub use clax_store::confirmations::CONFIRMATION_TTL_SECS;

pub use clax_core::confirm::{CONFIRMATION_HEADER, MAX_WAIT_SECS, PENDING};

/// A decision that waits on the owner's browser while an agent is live,
/// with what binds a confirmation to it: approving one of these approves
/// no other.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    /// Answering question `question_id` with `answers`.
    Answer { question_id: String, answers: Value },
    /// Pushing `artifact_id` to remote `remote_id` (its ID) in `mode`,
    /// with the push's `auto` (letting agents push new versions without
    /// asking), `forked_from` and `title` as sent.
    Push {
        artifact_id: String,
        remote_id: String,
        mode: String,
        auto: Option<bool>,
        forked_from: Option<String>,
        title: Option<String>,
    },
    /// Connecting this Clax to the Clax at `origin`, named `name`, as
    /// machine `machine` (each as sent).
    Connect {
        origin: String,
        name: Option<String>,
        machine: Option<String>,
    },
    /// Minting a credential for the Clax extension `extension_id`: the
    /// extension then acts as the owner's browser, so pairing it while an
    /// agent is live is itself confirmed.
    ExtensionMint { extension_id: String },
    /// Widening who may open `artifact_id`: `to` is `world` (its site), or
    /// the public copy's link access change `body` on remote `remote_id`.
    Widen {
        artifact_id: String,
        to: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        remote_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
    },
    /// Approving machine request `grant_id` as a new person named `name`
    /// with `rights`.
    ApproveGrant {
        grant_id: String,
        name: String,
        rights: Value,
    },
    /// Setting person `person`'s rights to `rights`.
    SetRights { person: String, rights: Value },
    /// Revoking person `person`.
    RevokePerson { person: String },
}

/// Proof that a route's decision may go ahead ([`require_confirmation`]).
#[derive(Debug)]
pub struct Confirmed(());

/// How far a request's browser credential is trusted to confirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Browser {
    /// None: the token alone.
    None,
    /// One an agent holding the token cannot have made: an owner browser
    /// session whose login code was minted while no agent session was
    /// live, or the in-process app's frame.
    Trusted,
    /// The extension's credential: it decides without asking, as before,
    /// but answers no confirmation (a token holder can mint one).
    Extension,
    /// One an agent holding the token could have made: the key cookie, or
    /// a session minted while an agent session was live.
    Untrusted,
}

/// What a request brings to [`require_confirmation`]: how far its browser
/// credential is trusted, and the confirmation it presents
/// ([`CONFIRMATION_HEADER`]).
#[derive(Clone, Debug)]
pub struct Confirming {
    browser: Browser,
    presented: Option<String>,
}

impl Confirming {
    /// Whether the request's browser credential may answer a confirmation:
    /// an owner browser session minted while no agent session was live, or
    /// the in-process app's frame.
    pub fn may_answer(&self) -> bool {
        self.browser == Browser::Trusted
    }
}

/// The trust of `parts`' browser credential, for a daemon whose live
/// owner sessions `s` holds.
fn browser_of(s: &AppState, parts: &Parts) -> Browser {
    let counts = crate::local_trust::session_counts(&parts.headers, &parts.extensions);
    if let Some(m) = parts
        .extensions
        .get::<crate::local_trust::OwnerSession>()
        .filter(|_| counts)
    {
        return match &m.session {
            Some(sha) if s.owner_sessions.minted_while_agent(sha) => Browser::Untrusted,
            Some(_) => Browser::Trusted,
            None if m.key => Browser::Untrusted,
            None => Browser::Trusted,
        };
    }
    if parts
        .extensions
        .get::<crate::extension::ViaExtension>()
        .is_some()
    {
        return Browser::Extension;
    }
    Browser::None
}

impl FromRequestParts<AppState> for Confirming {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(
        parts: &mut Parts,
        s: &AppState,
    ) -> Result<Self, std::convert::Infallible> {
        Ok(Confirming {
            browser: browser_of(s, parts),
            presented: parts
                .headers
                .get(CONFIRMATION_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        })
    }
}

/// Lets `what` go ahead for `who`, or answers why not yet (see the module
/// docs). A route calls it first, before it changes anything.
///
/// # Errors
/// 202 [`PENDING`] while the owner has not confirmed (a new confirmation,
/// or the one presented); 403 `declined` for one the owner declined; 410
/// `confirmation_expired` for one past its time; 409 `confirmation_used`
/// for one spent already; 403 `confirmation_mismatch` for one bound to
/// another decision; 404 `unknown_confirmation` for an ID none has; the
/// store's.
pub async fn require_confirmation(
    s: &AppState,
    who: &Caller,
    by: &Confirming,
    what: Decision,
) -> Result<Confirmed, ApiError> {
    let needs = matches!(who, Caller::Owner { .. })
        && !matches!(by.browser, Browser::Trusted | Browser::Extension);
    if s.hosted.is_some() || !needs {
        return Ok(Confirmed(()));
    }
    let decision = json!(what);
    let presented = by.presented.clone();
    let (st, base) = (s.clone(), s.browser_base.clone());
    let (outcome, announce) = s
        .store_call(move |db| {
            if !db.agent_session_live()? {
                return Ok((Ok(()), None));
            }
            let c = match presented {
                Some(id) => match db.use_confirmation(&id, &decision)? {
                    Use::Approved => return Ok((Ok(()), None)),
                    Use::Pending(c) => c,
                    Use::Declined => return Ok((Err(declined()), None)),
                    Use::Expired => return Ok((Err(expired()), None)),
                    Use::Used => return Ok((Err(used()), None)),
                    Use::Mismatch => return Ok((Err(mismatch()), None)),
                    Use::Unknown => return Ok((Err(unknown()), None)),
                },
                None => db.open_confirmation(&decision)?,
            };
            let v = view(&st, db, &c)?;
            Ok((Err(pending(&base, &c, &v)), Some(v)))
        })
        .await?;
    if let Some(v) = announce {
        s.events
            .publish(clax_core::Event::Confirmation { confirmation: v });
    }
    outcome.map(|()| Confirmed(()))
}

/// The 202 for confirmation `c`, whose view is `v`.
fn pending(base: &str, c: &Confirmation, v: &Value) -> ApiError {
    let mut f = serde_json::Map::new();
    f.insert("confirmation_id".into(), json!(c.id));
    f.insert("confirm_url".into(), json!(confirm_url(base, &c.id)));
    f.insert("decision".into(), v["words"].clone());
    f.insert("expires_at".into(), json!(c.expires_at));
    ApiError::accepted(PENDING, f)
}

/// Where the owner confirms `id`: the inbox, on the daemon's browser
/// address `base`, focused on its card.
pub fn confirm_url(base: &str, id: &str) -> String {
    format!("{base}/inbox?confirm={id}")
}

fn declined() -> ApiError {
    ApiError::forbidden("declined", "the owner declined this in their browser")
}

fn expired() -> ApiError {
    ApiError::new(
        StatusCode::GONE,
        "confirmation_expired",
        "this confirmation has expired: make the call again, without it, for a new one",
    )
}

fn used() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "confirmation_used",
        "this confirmation was used already: make the call again, without it, for a new one",
    )
}

fn mismatch() -> ApiError {
    ApiError::forbidden(
        "confirmation_mismatch",
        "this confirmation is for another decision",
    )
}

fn unknown() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "unknown_confirmation",
        "no confirmation has this ID",
    )
}

/// Confirmation `c` as the owner's browser and the token see it: its ID,
/// status and times, the decision as recorded and in words, its
/// `confirm_url`, and the agent sessions that make it wait (`agents`: the
/// owner's agents on this machine live now or ended within
/// [`CONFIRMATION_TTL_SECS`], each one's name, harness, handle, start and
/// end), which the owner checks before confirming.
///
/// # Errors
/// The store's.
pub fn view(s: &AppState, st: &Store, c: &Confirmation) -> clax_core::Result<Value> {
    let agents: Vec<Value> = st
        .live_agent_sessions()?
        .iter()
        .map(|a| {
            json!({
                "name": a.name,
                "harness": a.harness,
                "handle": a.agent_handle,
                "cwd": a.cwd,
                "started_at": a.started_at,
                "ended_at": a.ended_at,
            })
        })
        .collect();
    Ok(json!({
        "id": c.id,
        "status": c.status,
        "created_at": c.created_at,
        "expires_at": c.expires_at,
        "answered_at": c.answered_at,
        "used_at": c.used_at,
        "decision": c.decision,
        "words": words(st, &c.decision)?,
        "confirm_url": confirm_url(&s.browser_base, &c.id),
        "agents": agents,
    }))
}

/// The decision recorded as `d`, in words: what the card asks to confirm
/// ("Confirm: <words>").
///
/// # Errors
/// The store's.
pub fn words(st: &Store, d: &Value) -> clax_core::Result<String> {
    let s = |k: &str| d[k].as_str().unwrap_or_default().to_string();
    let title = |aid: &str| -> clax_core::Result<String> {
        Ok(match clax_core::ArtifactId::parse(aid) {
            Ok(id) => st
                .get_artifact(&id)?
                .map(|a| format!("“{}”", a.title))
                .unwrap_or_else(|| aid.to_string()),
            Err(_) => aid.to_string(),
        })
    };
    let remote = |rid: &str| -> clax_core::Result<String> {
        Ok(st
            .find_remote(rid)?
            .map(|r| r.origin)
            .unwrap_or_else(|| rid.to_string()))
    };
    Ok(match d["kind"].as_str().unwrap_or_default() {
        "answer" => {
            let qid = s("question_id");
            let asked = st
                .question(&qid)?
                .and_then(|q| q.questions.first().map(|x| x.question.clone()));
            match asked {
                Some(q) => format!("answer an agent's question “{q}”"),
                None => "answer an agent's question".to_string(),
            }
        }
        "push" => format!(
            "push {} to {}",
            title(&s("artifact_id"))?,
            remote(&s("remote_id"))?
        ),
        "connect" => format!("connect this Clax to {}", s("origin")),
        "widen" => match d["remote_id"].as_str() {
            Some(rid) => format!(
                "change who may open the public copy {} on {}",
                s("artifact_id"),
                remote(rid)?
            ),
            None => format!("open {}'s site to the world", title(&s("artifact_id"))?),
        },
        "approve_grant" => format!("connect a new machine as {}", s("name")),
        "extension_mint" => "pair the Clax extension, which then acts as your browser".to_string(),
        "set_rights" => "change a person's rights".to_string(),
        "revoke_person" => "revoke a person".to_string(),
        _ => "make a decision".to_string(),
    })
}

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
//! 3. connecting a remote, and answering an agent's connection request;
//! 4. widening who may open a page (its site to the world, its public
//!    copy's link access, a share of it with a person, its comments
//!    turned on);
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
//! the token and no browser credential, or with the app's key cookie (its
//! value is the token). Any owner browser session, the in-process app's
//! frame (the Clax app's shell, relayed through `clax_embed_call`) and a
//! paired extension decide at once, and any owner browser session or the
//! in-process frame may answer a confirmation. Every other caller (the
//! owner's agents, people and their agents) is left to the route, as
//! before: this adds no power to anyone. A hosted daemon, whose token no
//! local agent reads, asks for none.
//!
//! **External drivers only.** Confirmation guards the drivers outside the
//! Clax app: the localhost daemon's token (`<home>/daemon.json`, read by
//! the CLI, the plugins and agents) and the extension's minted credential.
//! The Clax app keeps its own trust: its core runs in the app's process
//! with no listener and no `daemon.json` (`clax-embed`), so no external
//! driver reaches it, and every request it serves ([`AppState::embed`]) is
//! the app's own code or its WebViews'. It asks for none, whatever sessions
//! are live there, the app's own agent's included. On a localhost daemon a
//! session named `app` counts like any other: only a holder of the token,
//! an external driver, can register one there.
//!
//! **Passkeys** (opt-in, [`clax_store::passkeys`]): the owner may require
//! a passkey to confirm (Touch ID, or any passkey: WebAuthn with user
//! verification, [`clax_core::webauthn`]). While that is on and a passkey
//! is registered ([`clax_store::Store::passkeys_required`]), approving a
//! confirmation from a browser session also takes an assertion over a
//! challenge bound to that confirmation ([`check_assertion`]), and a
//! browser session's or the extension's own decisions wait for a
//! confirmation too, since an agent holding the token can open a browser
//! session of its own. The in-process frame stays exempt: no agent reaches
//! it. Registering a passkey is itself confirmed from any browser while an
//! agent is live ([`Decision::RegisterPasskey`]). Passkeys work only where
//! the daemon's browser address is a secure context and a valid RP ID
//! (`localhost`, [`passkey_rp`]); elsewhere none can be registered, and
//! confirming stays as by default.

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
    /// Answering connection request `request_id` with `answer`
    /// (`connect`, `not_now` or `never`): a Connect connects this Clax.
    AnswerConnection { request_id: String, answer: String },
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
    /// Widening who may open `artifact_id`: `to` is `world` (its site), or,
    /// on remote `remote_id` with the change `body` as sent, the public
    /// copy's link access (`link`, `public`), a share with a person
    /// (`person`), or its comments turned on (`comments`).
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
    /// Registering passkey `credential_id` for confirmations: asked from
    /// any browser but the in-process frame while an agent is live, since
    /// a passkey becomes the authority on every confirmation. Bound to
    /// `body_sha256`, the SHA-256 of the whole registration
    /// ([`registration_sha256`]), so an approved registration lets in no
    /// other key under its credential ID.
    RegisterPasskey {
        credential_id: String,
        body_sha256: String,
    },
}

impl Decision {
    /// Whether it waits for a confirmation whatever browser makes it.
    fn asks_any_browser(&self) -> bool {
        matches!(self, Decision::RegisterPasskey { .. })
    }
}

/// Proof that a route's decision may go ahead ([`require_confirmation`]).
#[derive(Debug)]
pub struct Confirmed(());

/// The request's browser credential, as confirmations read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Browser {
    /// None: the token alone.
    None,
    /// The in-process app's frame, which no request from the network
    /// carries.
    InProcess,
    /// An owner browser session.
    Session,
    /// A paired extension's credential: it decides without asking (unless
    /// passkeys are required), and answers no confirmation.
    Extension,
    /// The key cookie, whose value is the token.
    Key,
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
    /// an owner browser session, or the in-process app's frame.
    pub fn may_answer(&self) -> bool {
        matches!(self.browser, Browser::Session | Browser::InProcess)
    }

    /// Whether answering takes a passkey's assertion when passkeys are
    /// required: a browser session's does; the in-process frame's does not.
    pub fn answers_with_passkey(&self) -> bool {
        self.browser == Browser::Session
    }
}

/// `parts`' browser credential.
fn browser_of(parts: &Parts) -> Browser {
    let counts = crate::local_trust::session_counts(&parts.headers, &parts.extensions);
    if let Some(m) = parts
        .extensions
        .get::<crate::local_trust::OwnerSession>()
        .filter(|_| counts)
    {
        return match &m.session {
            Some(_) => Browser::Session,
            None if m.key => Browser::Key,
            None => Browser::InProcess,
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
        _s: &AppState,
    ) -> Result<Self, std::convert::Infallible> {
        Ok(Confirming {
            browser: browser_of(parts),
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
    // Who may ask at all, before any store read: the owner, outside the
    // in-process frame, on a local daemon that external drivers reach (not
    // a hosted one, nor the Clax app's in-process core).
    if s.hosted.is_some()
        || s.embed.is_some()
        || !matches!(who, Caller::Owner { .. })
        || by.browser == Browser::InProcess
    {
        return Ok(Confirmed(()));
    }
    let browser = by.browser;
    let always = what.asks_any_browser();
    let decision = json!(what);
    let presented = by.presented.clone();
    let (st, base) = (s.clone(), s.browser_base.clone());
    let (outcome, announce) = s
        .store_call(move |db| {
            let needs = match browser {
                Browser::None | Browser::Key => true,
                Browser::Session | Browser::Extension => always || db.passkeys_required()?,
                Browser::InProcess => false,
            };
            if !needs || !db.agent_session_live()? {
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
        "answer_connection" => {
            let origin = st
                .remote_request(&s("request_id"))
                .map(|r| r.origin)
                .unwrap_or_else(|_| "a Clax instance".into());
            match s("answer").as_str() {
                "connect" => format!("connect this Clax to {origin}, as an agent asked"),
                "never" => format!("never connect this Clax to {origin}"),
                _ => format!("put off connecting this Clax to {origin}"),
            }
        }
        "widen" => match d["remote_id"].as_str() {
            Some(rid) if d["to"] == "comments" => format!(
                "let readers comment on the public copy {} on {}",
                s("artifact_id"),
                remote(rid)?
            ),
            Some(rid) if d["to"] == "person" => format!(
                "share the public copy {} on {} with someone",
                s("artifact_id"),
                remote(rid)?
            ),
            Some(rid) => format!(
                "change who may open the public copy {} on {}",
                s("artifact_id"),
                remote(rid)?
            ),
            None => format!("open {}'s site to the world", title(&s("artifact_id"))?),
        },
        "approve_grant" => format!("connect a new machine as {}", s("name")),
        "extension_mint" => "pair the Clax extension, which then acts as your browser".to_string(),
        "register_passkey" => "add a passkey that confirms these decisions from now on".to_string(),
        "set_rights" => "change a person's rights".to_string(),
        "revoke_person" => "revoke a person".to_string(),
        _ => "make a decision".to_string(),
    })
}

/// The RP ID and origin passkeys are registered and asserted for: the
/// daemon's browser address (`http://localhost:<port>`), when it is a
/// secure context with a valid RP ID (a `localhost` host). `None`
/// elsewhere (a LAN address, an IP literal): no passkey can be used there.
pub fn passkey_rp(s: &AppState) -> Option<(String, String)> {
    let origin = s.browser_base.trim_end_matches('/').to_string();
    let host = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))?;
    let rp = host.rsplit_once(':').map_or(host, |(h, _)| h).to_string();
    (rp == "localhost" || rp.ends_with(".localhost")).then_some((rp, origin))
}

/// The SHA-256 (lowercase hex) of a registration's fields, each as sent,
/// one per line: what [`Decision::RegisterPasskey`] binds.
pub fn registration_sha256(
    credential_id: &str,
    public_key: &str,
    alg: i64,
    authenticator_data: &str,
    client_data_json: &str,
) -> String {
    clax_core::secret::sha256_hex(&format!(
        "{credential_id}\n{public_key}\n{alg}\n{authenticator_data}\n{client_data_json}"
    ))
}

/// A passkey's assertion, as the browser hands it over (each field
/// base64url).
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    pub credential_id: String,
    pub authenticator_data: String,
    pub client_data_json: String,
    pub signature: String,
}

/// 403 `passkey_required`: confirming takes a passkey's assertion now.
pub fn passkey_required() -> ApiError {
    ApiError::forbidden(
        "passkey_required",
        "confirming needs your passkey (Touch ID or another): confirm in Clax's inbox",
    )
}

/// Checks assertion `a` for `purpose` (`confirm:<id>`, `disable`,
/// `remove:<credential ID>`): its challenge was issued for that purpose and
/// is spent now, and the passkey's signature, user verification and
/// counter check ([`clax_core::webauthn::verify_assertion`]). Runs on a
/// store thread.
///
/// # Errors
/// 403 `passkey_refused` with the reason, `passkey_required` where
/// passkeys cannot be used; the store's.
pub fn check_assertion(
    s: &AppState,
    st: &Store,
    purpose: &str,
    a: &Assertion,
) -> clax_core::Result<()> {
    use clax_core::webauthn::{Expect, challenge_of, from_b64url, verify_assertion};
    let refused = |why: &str| clax_core::CoreError::invalid("passkey_refused", why.to_string());
    let Some((rp_id, origin)) = passkey_rp(s) else {
        return Err(refused("passkeys work only at http://localhost"));
    };
    let decode = |v: &str| from_b64url(v).map_err(refused);
    let (ad, cdj, sig) = (
        decode(&a.authenticator_data)?,
        decode(&a.client_data_json)?,
        decode(&a.signature)?,
    );
    let challenge = challenge_of(&cdj).map_err(refused)?;
    if !st.take_passkey_challenge(&challenge, purpose)? {
        return Err(refused(
            "the challenge is not this one's, or was used or expired",
        ));
    }
    let e = Expect {
        origin: &origin,
        rp_id: &rp_id,
        challenge: &challenge,
    };
    st.use_passkey(&a.credential_id, |key, alg, n| {
        verify_assertion(&e, key, alg, n, &ad, &cdj, &sig)
    })
    .map_err(|err| match err {
        clax_core::CoreError::NotFound => refused("no passkey of this Clax has that ID"),
        other => other,
    })
}

/// The status a passkey refusal answers: 403.
pub fn passkey_error(e: ApiError) -> ApiError {
    match e.code {
        "passkey_refused" | "passkey_required" => ApiError::forbidden(e.code, e.message),
        "passkey_exists" => ApiError::new(StatusCode::CONFLICT, e.code, e.message),
        _ => e,
    }
}

//! Joins (`docs/contract.md`, "ClaxBot surfaces", "Joins"): a host's
//! standing request that its surface's operations be bound into ClaxBot's
//! own runs on artifacts it names.
//!
//! - `PUT /api/bot/surfaces/<name>/joins` (a [`JoinsRequest`]) → the
//!   joins the surface has now, as [`JoinsRequest`] writes them: replaces
//!   them whole ([`clax_store::Store::replace_joins`]). Who may: the
//!   daemon's token and nothing else, as every route of the protocol
//!   ([`clax_http::bot_surfaces::DaemonToken`]); the artifacts are ones
//!   the token's person made.
//! - [`joined_bindings`]: what [`super::bot::start_run`] makes a run of
//!   Clax's own kind with: the joins on its artifact whose registration
//!   acts for the run's person and is connected now, each bound with the
//!   operations its join gives ([`clax_bot::joins`]). With none, the run
//!   is made as it always was.
//! - [`joins_listed`]: each registration's joins, for `GET
//!   /api/bot/surfaces`.
//! - [`for_reader_of_hosts`], [`event_for_reader_of_hosts`]: what a
//!   reader of a run's artifact sees of a run a host's surface is bound
//!   into (the design's reader views).

use super::bot::Sender;
use clax_core::Result;
use clax_http::bot::Bot;
use clax_store::Store;
use clax_store::bot_joins::{JOINED_CLAX, Join};
use clax_store::bot_runs::Bindings;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The most joins one surface has: the artifacts a `PUT` names.
pub const MAX_JOINS: usize = 256;

/// The body of `PUT /api/bot/surfaces/<name>/joins`, and its answer: the
/// registration that sets them, and, for the one surface whose runs a
/// join can join yet (Clax's own), its scopes: each an artifact, with the
/// allow patterns that narrow what the join binds, within what the
/// registration's `allow` setting gives (left out: the definition's reads
/// that do not wait alone; a write, an effect or a read that waits is
/// bound only when the scope's `allow` names it, `clax_bot::joins`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JoinsRequest {
    pub registration: String,
    pub clax: Vec<ClaxScope>,
}

/// One scope of Clax's own surface a host joins: an artifact.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClaxScope {
    pub artifact: String,
    /// Left out, the join's default; `null` is refused, as it is in
    /// every other message of the protocol: it is not "left out".
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub allow: Option<Vec<String>>,
}

/// A member that is present: its value read as `T`, so that a `null`
/// where `T` takes none is refused.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// The scopes `joins` hold, as [`JoinsRequest::clax`] writes them.
fn scopes(joins: &[Join]) -> Vec<ClaxScope> {
    joins
        .iter()
        .filter(|j| j.joined == JOINED_CLAX)
        .map(|j| ClaxScope {
            artifact: j.artifact_id.clone(),
            allow: j.allow.clone(),
        })
        .collect()
}

/// The bindings a run of Clax's own kind sent by `sender` on artifact
/// `aid` for the threads `thread_ids`, with web grant `web`, is made
/// with when joins on `aid` bind
/// into it (`clax_bot::joins::bindings_with`); `None` when none does,
/// and the run is made as a run with no join is. Nothing is asked of the
/// hub when the host keeps none ([`Bot::joins`]), when an agent of
/// another machine sent the run under a grant ([`Sender::agent_grant`]:
/// a grant bounds Clax's own operations and names no host's, so such a
/// run is joined by none), or when no join is kept on the artifact. A
/// join binds only into a run that acts for the person its registration
/// was made by, only while that registration is connected, and only
/// when the run's threads hold no one's words but that person's
/// ([`only_the_senders_words`]).
///
/// # Errors
/// The store's.
pub fn joined_bindings(
    st: &Store,
    bot: &Bot,
    sender: &Sender,
    aid: &str,
    web: &str,
    thread_ids: &[String],
) -> Result<Option<Bindings>> {
    let Some(binder) = &bot.joins else {
        return Ok(None);
    };
    if sender.agent_grant.is_some() {
        return Ok(None);
    }
    let person = sender.person.as_str();
    let joins = st.joins_on(aid, JOINED_CLAX)?;
    if joins.is_empty() {
        return Ok(None);
    }
    let mut theirs = Vec::with_capacity(joins.len());
    for join in joins {
        let registrant = st.registration(&join.registration_id)?;
        if registrant.is_some_and(|r| r.registered_by == person) {
            theirs.push(join);
        }
    }
    if theirs.is_empty() {
        return Ok(None);
    }
    if !only_the_senders_words(st, person, thread_ids)? {
        tracing::debug!(
            "clax-bot: a run on {aid} is made with no join: its threads hold a comment by \
             someone other than the person it acts for"
        );
        return Ok(None);
    }
    let scope = clax_bot::joins::joined_scope(aid, thread_ids);
    Ok(clax_bot::joins::bindings_with(
        aid,
        web,
        binder.joined(&theirs, &scope),
    ))
}

/// Whether the threads `thread_ids` of a run that acts for `person` (a
/// person's public ID, or [`clax_store::roles::OWNER`]) hold no comment
/// but that person's own and ClaxBot's replies: the only runs a join
/// binds into ([`joined_bindings`]). The model of a joined run reads its
/// request threads beside the host's operations, which act on the
/// person's machine or product and read their private data; a comment
/// anyone else wrote there is text they chose, which could ask for a
/// host's operation or for what one reads, and so a run whose threads
/// hold one is made as a run with no join. The person's own comment is
/// one they wrote as a viewer under their own public ID, not through a
/// page (`via_page`); ClaxBot's reply is an agent comment of no session
/// under ClaxBot's name ([`clax_http::bot::NAME`], or "ClaxBot (for
/// <name>)"). Every other comment, a thread that is not there, and an
/// owner that is not known count as someone else's words. With no
/// threads, `true`.
///
/// # Errors
/// The store's.
pub fn only_the_senders_words(st: &Store, person: &str, thread_ids: &[String]) -> Result<bool> {
    use clax_core::model::{AUTHOR_AGENT, AUTHOR_VIEWER};
    let senders = if person == clax_store::roles::OWNER {
        st.owner()?.map(|v| v.public_id)
    } else {
        Some(person.to_string())
    };
    let Some(senders) = senders else {
        return Ok(thread_ids.is_empty());
    };
    let claxbots = format!("{} (for ", clax_http::bot::NAME);
    for tid in thread_ids {
        let Some(thread) = st.get_thread(tid)? else {
            return Ok(false);
        };
        let all_theirs = thread.comments.iter().all(|c| {
            let by_sender = c.author_kind == AUTHOR_VIEWER
                && !c.via_page
                && c.author_public_id.as_deref() == Some(senders.as_str());
            let by_claxbot = c.author_kind == AUTHOR_AGENT
                && c.via_harness.is_none()
                && c.agent.is_none()
                && (c.author_name == clax_http::bot::NAME || c.author_name.starts_with(&claxbots));
            by_sender || by_claxbot
        });
        if !all_theirs {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `listed`, the registrations `GET /api/bot/surfaces` lists, each with
/// `joins`: what it joins, `{clax: [{artifact, allow?}]}`.
///
/// # Errors
/// The store's.
pub fn joins_listed<T: Serialize>(st: &Store, listed: Vec<T>) -> Result<Vec<Value>> {
    listed
        .into_iter()
        .map(|entry| {
            let mut v = serde_json::to_value(entry).unwrap_or(Value::Null);
            let name = v["name"].as_str().unwrap_or_default().to_string();
            let registration = v["registration"].as_str().unwrap_or_default().to_string();
            let mine: Vec<Join> = st
                .joins_of(&name)?
                .into_iter()
                .filter(|j| j.registration_id == registration)
                .collect();
            v["joins"] = json!({ "clax": scopes(&mine) });
            Ok(v)
        })
        .collect()
}

/// Whether a host's surface can be bound into a run on this daemon: it
/// keeps joins ([`Bot::joins`]) and is not hosted. A hosted daemon (the
/// Worker) binds no registration, so [`hides_hosts`] reads nothing there.
#[must_use]
pub fn hosts_bind(bot: &Bot, hosted: bool) -> bool {
    !hosted && bot.joins.is_some()
}

/// Whether a reader of run `id`'s artifact may see no more of it than
/// that it happened, its kind, state and times and the finish's summary:
/// the run has a host's surface bound (a binding that names a
/// registration), or bindings this build cannot read (a later build's
/// version, or text that is no bindings), which may hold one. Decided
/// from the run's row alone ([`Store::run_as_made`]), never from the
/// registrations a hub holds now, so a reader sees the same whether or
/// not the host is connected. No registration can let a reader see more
/// yet: one that gives `reader_view` is refused (`invalid_settings`).
/// `false`, with nothing read, on a daemon where no host binds
/// (`binds`, [`hosts_bind`]); `false` for a run of bindings of version
/// 1, which hold Clax's own surface alone, and for a run that is not
/// there.
///
/// # Errors
/// The store's.
pub fn hides_hosts(st: &Store, binds: bool, id: &str) -> Result<bool> {
    hides_hosts_of(binds, || Ok(st.run_as_made(id)?.map(|(_, made)| made)))
}

/// [`hides_hosts`], with `made` the read of the run's row: called only
/// when `binds`.
fn hides_hosts_of(
    binds: bool,
    made: impl FnOnce() -> Result<Option<clax_store::bot_runs::RunAsMade>>,
) -> Result<bool> {
    if !binds {
        return Ok(false);
    }
    let Some(made) = made()? else {
        return Ok(false);
    };
    Ok(made.bindings().map_or(true, |bindings| {
        bindings.version() != clax_store::bot_runs::BINDINGS_VERSION
            && bindings.all().iter().any(|b| b.registration().is_some())
    }))
}

/// What of a step a reader of a run that a host's surface is bound into
/// sees when it shows a reader nothing ([`hides_hosts`]): its place,
/// kind, state and times, and a finish's summary and sources (the reply
/// readers of the artifact see anyway) or the reason a run stopped or
/// failed (a code); no title, no call, no figure, and not the reason a
/// `give_up` step gives, which is the model's own words and may repeat
/// what a host's call answered. Applied after [`super::bot::for_reader`].
pub fn for_reader_of_hosts(step: &mut Value) {
    const SHOWN: [&str; 8] = [
        "n",
        "kind",
        "status",
        "started_at",
        "ended_at",
        "summary",
        "sources",
        "reason",
    ];
    if let Some(o) = step.as_object_mut() {
        let gave_up = o.get("kind").and_then(Value::as_str) == Some("give_up");
        o.retain(|k, _| SHOWN.contains(&k.as_str()) && !(gave_up && k == "reason"));
    }
}

/// What of run view `v` ([`super::bot::run_view`]) such a reader sees
/// ([`hides_hosts`]): all of it but what names a host's surface. A
/// parked run's `waiting_on` keeps when it parked (`since`) and its
/// `deadline`, and loses the `surface` and the `op` it waits on.
pub fn run_for_reader_of_hosts(v: &mut Value) {
    if let Some(on) = v.get_mut("waiting_on").and_then(Value::as_object_mut) {
        on.retain(|k, _| k == "since" || k == "deadline");
    }
}

/// Event `data` of `event` as such a reader sees it ([`for_reader_of_hosts`],
/// [`run_for_reader_of_hosts`]), given what a reader of the artifact sees
/// of it: `None` for a script's title and a call.
#[must_use]
pub fn event_for_reader_of_hosts(event: &str, mut data: Value) -> Option<Value> {
    match event {
        "script" | "call" => None,
        "step_started" | "step_done" | "run_end" => {
            if event != "run_end" {
                for_reader_of_hosts(&mut data["step"]);
            }
            if data.get("run").is_some() {
                run_for_reader_of_hosts(&mut data["run"]);
            }
            Some(data)
        }
        _ => Some(data),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use route::put;

#[cfg(not(target_arch = "wasm32"))]
mod route {
    use super::{ClaxScope, JoinsRequest, MAX_JOINS, scopes};
    use crate::routes::artifacts::path;
    use axum::Json;
    use axum::body::Bytes;
    use axum::extract::rejection::{BytesRejection, PathRejection};
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use clax_bot::bot_surfaces::{ServeError, allow_patterns};
    use clax_core::CoreError;
    use clax_http::agent::DeniedToGrants;
    use clax_http::bot_surfaces::{DaemonToken, refusal, refused};
    use clax_http::error::ApiError;
    use clax_http::state::AppState;
    use clax_store::bot_joins::{
        CODE_JOIN_ARTIFACT_REFUSED, CODE_JOIN_ARTIFACT_UNKNOWN, JOINED_CLAX, NewJoin,
    };
    use clax_surface::wire::CODE_INVALID_REQUEST;

    /// `PUT /api/bot/surfaces/<name>/joins` (see the module
    /// documentation): a body that is not a [`JoinsRequest`] is 400
    /// `invalid_request`, and one longer than the hub's `request_bytes`
    /// 413 `too_large`; the registration is one the hub holds of the
    /// surface `name` (else 404 `unknown_registration`); there are
    /// [`MAX_JOINS`] artifacts at the most, each named once, each `allow`
    /// a list of allow patterns (else 400 `invalid_request`); each
    /// artifact is one here and not deleted, by its ID (else 404
    /// `join_artifact_unknown`), and was made by the token's person (else
    /// 403 `join_artifact_refused`). Nothing is changed by a request that
    /// is refused. The answer is the joins the surface has now.
    pub async fn put(
        State(s): State<AppState>,
        token: DaemonToken,
        _d: DeniedToGrants,
        p: Result<Path<String>, PathRejection>,
        body: Result<Bytes, BytesRejection>,
    ) -> Result<Json<JoinsRequest>, ApiError> {
        let name = path(p)?;
        let surfaces = s.bot_surfaces.clone();
        let limits = surfaces.limits();
        let b: JoinsRequest = crate::routes::bot_surfaces::message(body, limits.request_bytes)?;
        let invalid = |m: String| refused(CODE_INVALID_REQUEST, m);
        if b.clax.len() > MAX_JOINS {
            return Err(invalid(format!(
                "a surface joins {MAX_JOINS} artifacts at the most, and the request names {}",
                b.clax.len()
            )));
        }
        let mut joins = Vec::with_capacity(b.clax.len());
        for ClaxScope { artifact, allow } in &b.clax {
            if let Some(list) = allow {
                allow_patterns(list, &limits.definition).map_err(&invalid)?;
            }
            if joins.iter().any(|j: &NewJoin| &j.artifact_id == artifact) {
                return Err(invalid(format!("the artifact {artifact} is named twice")));
            }
            joins.push(NewJoin {
                joined: JOINED_CLAX.to_string(),
                artifact_id: artifact.clone(),
                allow: allow.clone(),
            });
        }
        // The token's person is the owner, whose artifacts are those
        // made by no other person.
        let maker = (token.person() != clax_store::roles::OWNER).then(|| token.person());
        let registration = b.registration.clone();
        let set = s
            .store_call(move |st| {
                if surfaces.find(&name, &registration).is_err() {
                    return Ok(Err(refusal(ServeError::UnknownRegistration.body())));
                }
                let now = st.now_utc();
                match st.replace_joins(&name, &registration, maker, &joins, now) {
                    Ok(set) => Ok(Ok(set)),
                    Err(CoreError::Invalid { code, message }) => Ok(Err(match code {
                        CODE_JOIN_ARTIFACT_UNKNOWN => {
                            ApiError::new(StatusCode::NOT_FOUND, code, message)
                        }
                        CODE_JOIN_ARTIFACT_REFUSED => {
                            ApiError::new(StatusCode::FORBIDDEN, code, message)
                        }
                        _ => refused(CODE_INVALID_REQUEST, message),
                    })),
                    Err(CoreError::NotFound) => {
                        Ok(Err(refusal(ServeError::UnknownRegistration.body())))
                    }
                    Err(e) => Err(e),
                }
            })
            .await??;
        Ok(Json(JoinsRequest {
            registration: b.registration,
            clax: scopes(&set),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_of_a_run_with_a_hosts_surface_sees_its_steps_bare() {
        let mut script = json!({
            "n": 2, "kind": "script", "status": "done", "started_at": "a", "ended_at": "b",
            "credits_milli": 3, "title": "Read the shell's windows",
            "calls": [{"op": "shell.windows", "ok": true, "ms": 0}],
        });
        super::super::bot::for_reader(&mut script);
        for_reader_of_hosts(&mut script);
        assert_eq!(
            script,
            json!({"n": 2, "kind": "script", "status": "done", "started_at": "a", "ended_at": "b"})
        );
        let mut finish = json!({"n": 3, "kind": "finish", "status": "done", "summary": "Done.",
                                "sources": ["https://example.com"], "credits_milli": 1});
        for_reader_of_hosts(&mut finish);
        assert_eq!(
            finish,
            json!({"n": 3, "kind": "finish", "status": "done", "summary": "Done.",
                   "sources": ["https://example.com"]})
        );
        assert_eq!(
            event_for_reader_of_hosts("script", json!({"title": "x"})),
            None
        );
        assert_eq!(event_for_reader_of_hosts("call", json!({"call": {}})), None);
        let step = event_for_reader_of_hosts("step_done", json!({"step": {"n": 1, "title": "x"}}));
        assert_eq!(step, Some(json!({"step": {"n": 1}})));
        assert_eq!(
            event_for_reader_of_hosts("run_end", json!({"run": {"status": "finished"}})),
            Some(json!({"run": {"status": "finished"}}))
        );
        // A parked run says that it waits and since when, not on what.
        let parked = || {
            json!({"status": "parked", "waiting_on": {"surface": "desk", "op": "desk.ask",
                   "since": "a", "deadline": "b"}})
        };
        let bare = json!({"status": "parked", "waiting_on": {"since": "a", "deadline": "b"}});
        let mut run = parked();
        run_for_reader_of_hosts(&mut run);
        assert_eq!(run, bare);
        for event in ["step_done", "run_end"] {
            let data = event_for_reader_of_hosts(event, json!({"step": {"n": 1}, "run": parked()}));
            assert_eq!(data.unwrap()["run"], bare, "{event}");
        }
    }

    /// On a daemon where no host binds (hosted, or keeping no joins) the
    /// run's row is never read; elsewhere bindings of version 1 hide
    /// nothing, bindings that do not read hide all, and a binding that
    /// names a registration hides the hosts.
    #[test]
    fn hides_hosts_reads_nothing_where_no_host_binds() {
        assert!(!hosts_bind(&Bot::default(), false));
        let unread = || -> Result<Option<clax_store::bot_runs::RunAsMade>> {
            panic!("the run's row is read")
        };
        assert!(!hides_hosts_of(false, unread).unwrap());
        let made = |bindings: String| clax_store::bot_runs::RunAsMade {
            kind: "clax.thread".into(),
            bindings_json: bindings,
            request_json: "{}".into(),
        };
        let hides = |bindings: String| hides_hosts_of(true, || Ok(Some(made(bindings)))).unwrap();
        assert!(!hides(Bindings::clax("a1", "").to_json()));
        assert!(hides(json!({"v": 99, "bindings": []}).to_string()));
        let joined = clax_bot::joins::bindings_with(
            "a1",
            "",
            vec![clax_store::bot_runs::StoredBinding::bound(
                "desk",
                Some("h".into()),
                Some("r1".into()),
                None,
                Value::Null,
                vec!["desk.lamps".into()],
            )],
        )
        .unwrap();
        assert!(hides(joined.to_json()));
        assert!(!hides_hosts_of(true, || Ok(None)).unwrap());
    }

    #[test]
    fn a_joins_body_is_its_registration_and_clax_scopes_and_nothing_else() {
        let b: JoinsRequest = serde_json::from_value(json!({
            "registration": "r", "clax": [{"artifact": "a"}, {"artifact": "b", "allow": ["desk.lamps"]}]
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&b).unwrap(),
            json!({"registration": "r", "clax": [{"artifact": "a"}, {"artifact": "b", "allow": ["desk.lamps"]}]})
        );
        for refused in [
            json!({"registration": "r"}),
            json!({"registration": "r", "clax": [], "shell": []}),
            json!({"registration": "r", "clax": [{"artifact": "a", "scope": 1}]}),
        ] {
            assert!(
                serde_json::from_value::<JoinsRequest>(refused.clone()).is_err(),
                "{refused}"
            );
        }
    }
}

//! The wire between a host of runs and a workspace service.
//!
//! JSON over HTTP, with a bearer secret the [`crate::Transport`] adds. Every
//! request carries [`WIRE_HEADER`] and [`WIRE_INSTANCE_HEADER`]. After the
//! secret, a service that speaks another version answers 426 with a
//! [`WireFailure`], whatever the route, and one that serves another instance
//! answers 403.
//!
//! | Request | Body | Answer |
//! |---|---|---|
//! | `POST /v1/jobs` | [`JobRequest`] | [`JobAnswer`], at once |
//! | `GET /v1/jobs/{key}?wait=20` | | [`JobAnswer`] within `wait` seconds, or 404 |
//! | `DELETE /v1/runs/{id}` | | [`Snapshot`], within 25 seconds |
//! | `POST /v1/reap` | [`ReapRequest`] | [`Snapshot`] |
//! | `GET /v1/previews` | | [`PreviewsAnswer`] |
//! | `POST /v1/previews/ack` | [`PreviewsAck`] | 204 |
//!
//! Every request may be sent again. A job is known by its key: a key seen
//! before joins the job, running or ended, and starts nothing. A job goes on
//! whoever is waiting for it, and the service keeps its answer until its run
//! is ended, so an answer lost on the way is asked for again. The exceptions
//! are these. A proposal that failed [`JobState::Failed::retryable`] is
//! forgotten, so its key may be sent again. A POST of a command's key the
//! service knew and no longer has a job for answers
//! [`JobState::Failed`] with `shell_interrupted`, status 200, and never runs
//! it again; a proposal's key is not refused so, since a proposal sent again
//! runs again and pushes nothing twice.
//!
//! Ending a run aborts its commands at once, and its proposals that have not
//! reached their push, and lets one past its push finish. The answer comes
//! within 25 seconds, under what a host waits for a request, and the ending
//! goes on after that, for at most 300 seconds.
//!
//! A poll of a job the service does not have answers 404 with a
//! [`WireFailure`] whose code is [`CODE_NO_SUCH_JOB`], begun or ended. Only
//! that says so: a tunnel on the way has 404s of its own.
//!
//! A request the service will not take answers a 4xx with a [`WireFailure`]
//! whose code is one of [`REFUSALS`]. That is its final word on the request:
//! a tunnel's own 401 or page of HTML is not, and says nothing of a job.
//!
//! Reaping ends everything not in [`ReapRequest::active`] under the service's
//! labels, so one service, or one label, serves one instance.
//!
//! A preview's events stay with the service until acknowledged, which holds
//! no credential into Clax: the host polls, and tells its own runs.
//!
//! The wire fails closed. [`Offer`] defaults to what a daemon's old files
//! need; on the wire it has a form of its own ([`OfferForm`]), every field
//! required and none unknown, and with no word on confidentiality it is
//! confidential. A [`Snapshot`] that cannot be read is no snapshot. A field
//! added to [`Offer`] changes this wire, so [`WIRE_VERSION`] changes with it.

use clax_bot::workspace::Offer;
use clax_store::bot_runs::{RequestKind, Run, RunStatus};
use clax_store::credits::KeySource;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The version of this wire.
pub const WIRE_VERSION: u32 = 1;
/// The request header that carries [`WIRE_VERSION`].
pub const WIRE_HEADER: &str = "clax-workspace-wire";
/// The request header that names the instance asking.
pub const WIRE_INSTANCE_HEADER: &str = "clax-workspace-instance";
/// The status of an answer to a request of another version.
pub const STATUS_WIRE_SKEW: u16 = 426;
/// The most seconds a job's poll waits. A Worker allows six connections
/// waiting for headers, so a poll is short and there are few at once.
pub const MAX_WAIT_SECS: u64 = 20;

/// The most bytes of an answer a host reads, and of a body a transport need
/// keep. A command's output is clipped well under it.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
/// The most preview events a host takes from one answer: the oldest, the rest
/// at the next poll.
pub const MAX_PREVIEW_EVENTS: usize = 100;
/// [`WireFailure::code`] of a poll of a job the service does not have.
pub const CODE_NO_SUCH_JOB: &str = "no_such_job";
/// [`WireFailure::code`] of a request without the service's secret.
pub const CODE_UNAUTHORIZED: &str = "unauthorized";
/// [`WireFailure::code`] of a request of another [`WIRE_VERSION`].
pub const CODE_WIRE_SKEW: &str = "wire_skew";
/// [`WireFailure::code`] of a request naming an instance the service does not
/// serve.
pub const CODE_WRONG_INSTANCE: &str = "wrong_instance";
/// [`WireFailure::code`] of a request whose ID or key is not one.
pub const CODE_BAD_ID: &str = "bad_id";
/// [`WireFailure::code`] of a request that could not be read.
pub const CODE_BAD_REQUEST: &str = "bad_request";
/// [`WireFailure::code`] of a request whose body is over the limit.
pub const CODE_TOO_LARGE: &str = "too_large";
/// [`WireFailure::code`] of a key that is another run's.
pub const CODE_KEY_TAKEN: &str = "key_taken";
/// Every code by which a service refuses a request for good.
pub const REFUSALS: [&str; 7] = [
    CODE_UNAUTHORIZED,
    CODE_WIRE_SKEW,
    CODE_WRONG_INSTANCE,
    CODE_BAD_ID,
    CODE_BAD_REQUEST,
    CODE_TOO_LARGE,
    CODE_KEY_TAKEN,
];

pub const JOBS_PATH: &str = "/v1/jobs";
pub const REAP_PATH: &str = "/v1/reap";
pub const PREVIEWS_PATH: &str = "/v1/previews";
pub const PREVIEWS_ACK_PATH: &str = "/v1/previews/ack";

/// `text` as one segment of a path.
fn segment(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

/// Where job `key` is polled.
#[must_use]
pub fn job_path(key: &str) -> String {
    format!("{JOBS_PATH}/{}?wait={MAX_WAIT_SECS}", segment(key))
}

/// Where run `id` is ended.
#[must_use]
pub fn run_path(id: &str) -> String {
    format!("/v1/runs/{}", segment(id))
}

/// What a service reads of a run. It has no store: a run is these and no more.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RunRef {
    pub id: String,
    pub thread_ids: Vec<String>,
}

impl RunRef {
    #[must_use]
    pub fn of(run: &Run) -> RunRef {
        RunRef {
            id: run.id.clone(),
            thread_ids: run.thread_ids.clone(),
        }
    }

    /// A [`Run`] with this ID and these threads, for a service's
    /// [`clax_bot::Workspace`], which reads no more of it. The rest is blank.
    #[must_use]
    pub fn to_run(&self) -> Run {
        Run {
            id: self.id.clone(),
            artifact_id: String::new(),
            person: String::new(),
            request: RequestKind::Thread,
            thread_ids: self.thread_ids.clone(),
            status: RunStatus::Running,
            reason: None,
            model: String::new(),
            key_source: KeySource::Instance,
            base_version: 0,
            credits_milli: 0,
            claimed_by: None,
            claimed_at: None,
            created_at: String::new(),
            updated_at: String::new(),
            ended_at: None,
            due_at: None,
            agent_grant: None,
            web: String::new(),
            confidential: false,
            woken_by: None,
        }
    }
}

/// A job to start, or to join if its key is known.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct JobRequest {
    /// The same on every attempt of one call of a run.
    pub key: String,
    pub run: RunRef,
    #[serde(flatten)]
    pub work: Work,
}

/// What a job does.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Work {
    /// [`clax_bot::Workspace::exec`]. It answers `{exit_code, output}`,
    /// `output` no longer than [`clax_bot::workspace::MAX_OUTPUT_BYTES`].
    Exec {
        queue: Option<String>,
        command: String,
        cwd: Option<String>,
        timeout_secs: u64,
    },
    /// [`clax_bot::Workspace::propose`], under the job's key.
    Propose { title: String, summary: String },
}

/// Where a job is.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobState {
    Running,
    /// It answered.
    Done {
        value: Value,
    },
    /// It answered with a [`clax_bot_sandbox::HostError`], which a run's
    /// script reads: its message holds nothing a script may not.
    Failed {
        code: String,
        message: String,
        #[serde(default)]
        retryable: bool,
    },
}

/// A service's answer on a job, with its offers as they stand.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct JobAnswer {
    #[serde(flatten)]
    pub state: JobState,
    pub snapshot: Snapshot,
}

/// An [`Offer`] as the wire has it: see the module documentation. Written out
/// by hand, and both ways by destructuring, so a field added to [`Offer`] does
/// not build until it is here.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OfferForm {
    pub shell: bool,
    pub change: bool,
    /// With no word it is confidential.
    #[serde(default = "confidential_unless_said")]
    pub confidential: bool,
    pub reviewed_previews: bool,
    pub wake_on_reply: bool,
    pub no_follow_ups: bool,
    pub answers_only: bool,
    pub unsure: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub guidance: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub preview_gate: Option<String>,
}

fn confidential_unless_said() -> bool {
    true
}

impl From<Offer> for OfferForm {
    fn from(offer: Offer) -> OfferForm {
        let Offer {
            shell,
            change,
            confidential,
            reviewed_previews,
            wake_on_reply,
            no_follow_ups,
            answers_only,
            unsure,
            guidance,
            preview_gate,
        } = offer;
        OfferForm {
            shell,
            change,
            confidential,
            reviewed_previews,
            wake_on_reply,
            no_follow_ups,
            answers_only,
            unsure,
            guidance,
            preview_gate,
        }
    }
}

impl From<OfferForm> for Offer {
    fn from(form: OfferForm) -> Offer {
        let OfferForm {
            shell,
            change,
            confidential,
            reviewed_previews,
            wake_on_reply,
            no_follow_ups,
            answers_only,
            unsure,
            guidance,
            preview_gate,
        } = form;
        Offer {
            shell,
            change,
            confidential,
            reviewed_previews,
            wake_on_reply,
            no_follow_ups,
            answers_only,
            unsure,
            guidance,
            preview_gate,
        }
    }
}

/// `serde(with)` for one offer in its [`OfferForm`].
mod one_offer {
    use super::{Deserialize, Deserializer, Offer, OfferForm, Serialize, Serializer};

    pub fn serialize<S: Serializer>(offer: &Offer, s: S) -> Result<S::Ok, S::Error> {
        OfferForm::from(offer.clone()).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Offer, D::Error> {
        OfferForm::deserialize(d).map(Offer::from)
    }
}

/// `serde(with)` for offers by ID, each in its [`OfferForm`].
mod offers {
    use super::{BTreeMap, Deserialize, Deserializer, Offer, OfferForm, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        offers: &BTreeMap<String, Offer>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let forms = offers
            .iter()
            .map(|(id, o)| (id, OfferForm::from(o.clone())));
        forms.collect::<BTreeMap<_, _>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, Offer>, D::Error> {
        let forms = BTreeMap::<String, OfferForm>::deserialize(d)?;
        Ok(forms.into_iter().map(|(id, f)| (id, f.into())).collect())
    }
}

/// Everything a service offers, so that [`clax_bot::Workspace::offer`], which
/// cannot wait, answers without asking. Every field is required.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Snapshot {
    /// Later snapshots have a greater or equal number, across a service's
    /// restarts: a host keeps the latest, whatever order answers came in.
    pub seq: u64,
    /// What a run gets, by ID, once it holds something: the offer it keeps.
    #[serde(with = "offers")]
    pub runs: BTreeMap<String, Offer>,
    /// What a run on a queue gets, by queue ID, for the queues the service
    /// has a target for. A target it could not read is [`Offer::unsure`].
    #[serde(with = "offers")]
    pub queues: BTreeMap<String, Offer>,
    /// What any other run gets: on no queue, or a queue with no target.
    #[serde(with = "one_offer")]
    pub others: Offer,
    /// The runs whose target says for certain that it is private, the
    /// service's own [`clax_bot::Workspace::target_private`].
    pub private_runs: BTreeSet<String>,
    /// The same of queues. A target that could not be read is in neither:
    /// what is not here is not known to be private, whatever its offer says.
    pub private_queues: BTreeSet<String>,
    /// Until when, in milliseconds since the epoch, a preview may yet be told
    /// of: the host goes on polling until then, whether or not its run has
    /// ended.
    #[serde(deserialize_with = "Option::deserialize")]
    pub previews_until_ms: Option<i64>,
}

impl Snapshot {
    /// What run `run`, on a report of `queue` if it is one, gets.
    #[must_use]
    pub fn offer(&self, run: &str, queue: Option<&str>) -> Offer {
        self.runs
            .get(run)
            .or_else(|| queue.and_then(|q| self.queues.get(q)))
            .unwrap_or(&self.others)
            .clone()
    }

    /// Whether run `run`'s target, on `queue` if it has one, says for certain
    /// that it is private.
    #[must_use]
    pub fn target_private(&self, run: &str, queue: Option<&str>) -> bool {
        self.private_runs.contains(run) || queue.is_some_and(|q| self.private_queues.contains(q))
    }

    /// Whether `other` says what this says, whatever its number.
    #[must_use]
    pub fn says_the_same(&self, other: &Snapshot) -> bool {
        self.runs == other.runs
            && self.queues == other.queues
            && self.others == other.others
            && self.private_runs == other.private_runs
            && self.private_queues == other.private_queues
            && self.previews_until_ms == other.previews_until_ms
    }
}

/// What to keep: see [`clax_bot::Workspace::reap`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReapRequest {
    pub active: Vec<String>,
}

/// What became of a fix's preview.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "told", rename_all = "snake_case")]
pub enum PreviewTold {
    /// [`clax_bot::workspace::PreviewSink::preview_ready`].
    Ready { verify_url: String, reviewed: bool },
    /// [`clax_bot::workspace::PreviewSink::preview_missed`].
    Missed,
}

/// One word of a preview, kept by the service until acknowledged.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PreviewEvent {
    /// Unique among a service's events.
    pub id: String,
    pub run: String,
    #[serde(flatten)]
    pub told: PreviewTold,
}

/// The events not yet acknowledged, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PreviewsAnswer {
    /// The instance these events are for, the service's own word.
    pub instance: String,
    pub events: Vec<PreviewEvent>,
    pub snapshot: Snapshot,
}

/// The events the host has told, which the service may forget.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PreviewsAck {
    pub ids: Vec<String>,
}

/// The body of an answer that is not a success.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WireFailure {
    pub code: String,
    pub message: String,
    /// The version the service speaks, on [`STATUS_WIRE_SKEW`].
    #[serde(default)]
    pub wire: Option<u32>,
}

impl WireFailure {
    /// The answer to a request whose [`WIRE_HEADER`] is not [`WIRE_VERSION`].
    #[must_use]
    pub fn skew() -> WireFailure {
        WireFailure {
            code: CODE_WIRE_SKEW.to_string(),
            message: format!("this service speaks wire version {WIRE_VERSION}"),
            wire: Some(WIRE_VERSION),
        }
    }
}

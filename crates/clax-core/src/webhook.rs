//! Report queues' outbound webhooks (spec 2026-10-27-clax-sdk-design §9):
//! the events, the payload a delivery carries, its signature, the retry
//! schedule and which endpoint URLs a queue may name. The outbox and the
//! endpoints are `clax_store::webhooks`'; the sending is the server's.
//!
//! # Signature
//!
//! Each delivery is a `POST` of the JSON [`Payload`] with
//! [`TIMESTAMP_HEADER`] (`Clax-Timestamp`, the send's Unix time in seconds)
//! and [`SIGNATURE_HEADER`] (`Clax-Signature: v1=<hex>`), the hex
//! HMAC-SHA256 under the endpoint's secret of `<timestamp>.<body>`, the
//! body's exact bytes. A receiver recomputes it, compares in constant time
//! and refuses a timestamp more than [`TOLERANCE_SECS`] from its own
//! clock ([`verify`]). A redelivery is signed anew at its own time, with
//! the same body and event ID.
//!
//! # Delivery
//!
//! At least once: a delivery is retried until the endpoint answers 2xx, at
//! most [`MAX_ATTEMPTS`] times, waiting [`backoff`] after each failure. A
//! receiver drops repeats by the payload's `id` (also sent as
//! [`EVENT_ID_HEADER`]).
//!
//! # Verification
//!
//! An endpoint is sent no report event until it proves that whoever
//! answers its URL holds its secret. Naming it sends one delivery of
//! [`VERIFY_EVENT`] (`webhook.verify`, a [`VerifyPayload`]) carrying a
//! random `challenge`, signed as every delivery is. The endpoint is
//! verified only when it answers 2xx with a JSON object of at most
//! [`MAX_ANSWER_BYTES`] whose `answer` is [`challenge_answer`]: the hex
//! HMAC-SHA256, under the secret, of `webhook.verify.<challenge>`
//! ([`check_answer`]). The verification is sent once, never retried on
//! the [`backoff`] schedule; an admin asks for another at most
//! [`VERIFY_SENDS_PER_HOUR`] times an hour, within [`VERIFY_WINDOW_SECS`]
//! of naming (or re-enabling) the endpoint. An endpoint past that window
//! unverified stays inert until it is removed.
//!
//! # Abuse limits
//!
//! [`DISABLE_AFTER`] deliveries in a row given up disable an endpoint: no
//! event is queued for it, its pending deliveries are given up, and
//! re-enabling it runs the verification again. An endpoint is queued at
//! most [`MAX_PER_HOUR`] events an hour; the rest are dropped for it and
//! counted.

use crate::error::CoreError;
use crate::queue::Report;
use crate::report::{FixOutcome, ReporterProfile, ResolveReason};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// What a webhook secret starts with.
pub const SECRET_PREFIX: &str = "whsec_";
/// What an event ID starts with.
pub const EVENT_PREFIX: &str = "evt_";
/// The header carrying `v1=<hex HMAC-SHA256>`.
pub const SIGNATURE_HEADER: &str = "Clax-Signature";
/// The header carrying the signed Unix time, in seconds.
pub const TIMESTAMP_HEADER: &str = "Clax-Timestamp";
/// The header naming the event's type (`report.created`).
pub const EVENT_HEADER: &str = "Clax-Event";
/// The header carrying the event's ID, the payload's `id`.
pub const EVENT_ID_HEADER: &str = "Clax-Event-Id";
/// How far a signed timestamp may be from the receiver's clock.
pub const TOLERANCE_SECS: i64 = 300;
/// Attempts of one delivery before it is given up and its endpoint marked
/// failing.
pub const MAX_ATTEMPTS: u32 = 8;
/// Endpoints one queue may name.
pub const MAX_ENDPOINTS: usize = 10;
/// Longest endpoint URL, in bytes.
pub const MAX_URL_BYTES: usize = 2048;
/// How long one attempt may take, answer included.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// How long a delivery taken for sending stays taken: one whose outcome
/// is never recorded (the process ended mid-send) is due again then.
pub const IN_FLIGHT_SECS: i64 = 60;
/// Deliveries one pass sends at most.
pub const BATCH: usize = 20;
/// Deliveries to one endpoint one pass sends at most, so the backlog of
/// an endpoint that never answers cannot fill a pass and hold back the
/// others'.
pub const PER_ENDPOINT: usize = 4;
/// Bytes of an answer's body (or an error's text) kept in the delivery log.
pub const MAX_LOGGED_BYTES: usize = 300;
/// Bytes of an answer's body a sender reads at most; a verification
/// answer longer than this is refused unread.
pub const MAX_ANSWER_BYTES: usize = 1024;
/// The event that verifies an endpoint ([`VerifyPayload`]).
pub const VERIFY_EVENT: &str = "webhook.verify";
/// Verification deliveries one endpoint is sent at most in an hour, the
/// one sent when it is named (or re-enabled) included.
pub const VERIFY_SENDS_PER_HOUR: usize = 5;
/// Verification deliveries one queue's endpoints are sent at most in an
/// hour, endpoints since removed included: removing and adding an
/// endpoint again sends no more.
pub const VERIFY_SENDS_PER_QUEUE_HOUR: usize = 30;
/// Verification deliveries sent at most in an hour to one host, from
/// every queue of the instance: a person with many queues sends no more.
pub const VERIFY_SENDS_PER_HOST_HOUR: usize = 60;
/// How long after it is named (or re-enabled) an endpoint may be
/// verified: past it, an unverified endpoint is inert until removed.
pub const VERIFY_WINDOW_SECS: i64 = 24 * 3600;
/// Deliveries in a row given up (each after [`MAX_ATTEMPTS`]) that
/// disable their endpoint.
pub const DISABLE_AFTER: u32 = 5;
/// Report events queued for one endpoint in an hour at most; the rest
/// are dropped for it and counted.
pub const MAX_PER_HOUR: u32 = 1000;

/// A report event an endpoint may be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Event {
    /// A report was filed.
    Created,
    /// An agent claimed it.
    Claimed,
    /// Its claim was released, by its holder or a triage person.
    Released,
    /// A triage person or the claiming agent replied to the reporter.
    Replied,
    /// A fix was proposed: it waits for the reporter's verdict.
    FixProposed,
    /// The reporter (or triage on their behalf) said the fix works.
    Verified,
    /// The reporter (or triage on their behalf) said it is still broken.
    Reopened,
    /// Triage closed it without a verdict.
    Resolved,
}

impl Event {
    /// Every event, in the order a report meets them.
    pub const ALL: &'static [Event] = &[
        Event::Created,
        Event::Claimed,
        Event::Released,
        Event::Replied,
        Event::FixProposed,
        Event::Verified,
        Event::Reopened,
        Event::Resolved,
    ];

    /// The event's type as payloads and endpoints name it.
    pub fn as_str(self) -> &'static str {
        match self {
            Event::Created => "report.created",
            Event::Claimed => "report.claimed",
            Event::Released => "report.released",
            Event::Replied => "report.replied",
            Event::FixProposed => "report.fix_proposed",
            Event::Verified => "report.verified",
            Event::Reopened => "report.reopened",
            Event::Resolved => "report.resolved",
        }
    }

    /// The event named `s`.
    pub fn parse(s: &str) -> Option<Event> {
        Event::ALL.iter().copied().find(|e| e.as_str() == s)
    }
}

/// `names` as events, each once, in [`Event::ALL`]'s order; every event
/// when `names` is empty.
///
/// # Errors
/// `invalid_events` naming the first that is no event.
pub fn parse_events(names: &[String]) -> Result<Vec<Event>, CoreError> {
    if names.is_empty() {
        return Ok(Event::ALL.to_vec());
    }
    let mut out = Vec::new();
    for n in names {
        let e = Event::parse(n).ok_or_else(|| {
            CoreError::invalid(
                "invalid_events",
                format!(
                    "{n:?} is not a webhook event; the events are {}",
                    Event::ALL
                        .iter()
                        .map(|e| e.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        })?;
        out.push(e);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// The signature header's value for `body` sent at Unix time `timestamp`
/// under `secret`: `v1=` and the hex HMAC-SHA256 of `<timestamp>.<body>`.
pub fn signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let mut msg = format!("{timestamp}.").into_bytes();
    msg.extend_from_slice(body);
    let mac = crate::secret::hmac_sha256(secret.as_bytes(), &msg);
    format!("v1={}", hex(&mac))
}

/// Why [`verify`] refused a delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// No timestamp header, or one that is no integer.
    BadTimestamp,
    /// The timestamp is more than [`TOLERANCE_SECS`] from `now`.
    Stale,
    /// No `v1=` signature matches.
    BadSignature,
}

/// Checks a delivery as a receiver does: `timestamp` within
/// [`TOLERANCE_SECS`] of `now` (Unix seconds), and one of the
/// comma-separated `v1=` values of `signature_header` equal, in constant
/// time, to [`signature`] of `body`.
///
/// # Errors
/// [`VerifyError`].
pub fn verify(
    secret: &str,
    timestamp: &str,
    signature_header: &str,
    body: &[u8],
    now: i64,
) -> Result<(), VerifyError> {
    let ts: i64 = timestamp
        .trim()
        .parse()
        .map_err(|_| VerifyError::BadTimestamp)?;
    if (now - ts).abs() > TOLERANCE_SECS {
        return Err(VerifyError::Stale);
    }
    let want = signature(secret, ts, body);
    let ok = signature_header
        .split(',')
        .map(str::trim)
        .filter(|v| v.starts_with("v1="))
        .any(|v| crate::secret::constant_time_eq(v.as_bytes(), want.as_bytes()));
    if ok {
        Ok(())
    } else {
        Err(VerifyError::BadSignature)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// How long a delivery waits after its `attempts`th failed attempt: one
/// minute after the first, four times longer after each next, at most six
/// hours (1 min, 4 min, 16 min, 64 min, 4 h 16 min, then 6 h): about
/// eighteen hours from the first attempt to the [`MAX_ATTEMPTS`]th.
pub fn backoff(attempts: u32) -> Duration {
    const CAP: u64 = 6 * 3600;
    let n = attempts.max(1) - 1;
    let secs = 4u64
        .checked_pow(n)
        .and_then(|m| m.checked_mul(60))
        .map_or(CAP, |s| s.min(CAP));
    Duration::from_secs(secs)
}

/// `raw` as an endpoint URL a queue may name: at most [`MAX_URL_BYTES`],
/// no user name, password or fragment, and `https`. A local Clax also
/// takes `http` to a loopback host (`localhost`, `127.0.0.0/8`, `[::1]`),
/// for development. A hosted Clax takes only a DNS name that is no IP
/// address in any spelling and names no private network
/// ([`crate::config::hosted_base_url_ok`]); what it resolves to is checked
/// again at each send.
///
/// # Errors
/// `invalid_url` for a URL that is none of these, saying why.
pub fn check_url(raw: &str, hosted: bool) -> Result<url::Url, CoreError> {
    let bad = |why: &str| CoreError::invalid("invalid_url", format!("{raw:?}: {why}"));
    if raw.len() > MAX_URL_BYTES {
        return Err(bad(&format!("longer than {MAX_URL_BYTES} bytes")));
    }
    let url = url::Url::parse(raw).map_err(|e| bad(&format!("not a URL ({e})")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("a webhook URL carries no user name or password"));
    }
    if url.fragment().is_some() {
        return Err(bad("a webhook URL carries no fragment"));
    }
    let Some(host) = url.host_str().filter(|h| !h.is_empty()) else {
        return Err(bad("it names no host"));
    };
    if hosted {
        if url.scheme() != "https" || !crate::config::hosted_base_url_ok(url.as_str(), false) {
            return Err(bad(
                "a hosted Clax sends webhooks only to https URLs of a public DNS name (no IP address, localhost or private name)",
            ));
        }
        return Ok(url);
    }
    match url.scheme() {
        "https" => Ok(url),
        "http" if loopback_host(host) => Ok(url),
        "http" => Err(bad("http is taken for a loopback host alone; use https")),
        _ => Err(bad("the scheme is neither https nor http")),
    }
}

/// Whether `host` (as a URL spells it) is `localhost` or a loopback
/// address.
fn loopback_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// What one delivery posts: the event, its queue and its report as triage
/// reads it, and for fix events the fix, for a reply the reply.
///
/// It carries the reporter's profile as the deployer's backend minted it
/// (name, email, GitHub login, avatar), so a relay can find the reporter
/// elsewhere (a Slack user by email): personal data, sent only to the
/// endpoints a queue's admins name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    /// The event's ID ([`EVENT_PREFIX`]), the same on every attempt and
    /// redelivery: a receiver drops repeats by it.
    pub id: String,
    /// The event ([`Event::as_str`]).
    #[serde(rename = "type")]
    pub kind: String,
    pub created_at: String,
    pub queue: PayloadQueue,
    pub report: PayloadReport,
    /// The latest fix: on `report.fix_proposed`, `report.verified` and
    /// `report.reopened`, when the report has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<PayloadFix>,
    /// The reply, on `report.replied`; the reopen's comment on
    /// `report.reopened`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<PayloadComment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadQueue {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadReport {
    /// The report's (its thread's) ID: what `?clax-report=<id>` names.
    pub id: String,
    pub state: crate::queue::State,
    pub external_id: String,
    pub artifact_id: String,
    pub title: String,
    /// The page it was filed on.
    pub page_url: String,
    /// The report in the Clax that holds it, for its queue's members; set
    /// when the delivery is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    pub reporter_id: Option<String>,
    /// The reporter's profile, when their token carried one.
    pub reporter: Option<ReporterProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve_reason: Option<ResolveReason>,
    pub reopen_count: u32,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadFix {
    pub n: u32,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_after: Option<String>,
    pub outcome: FixOutcome,
    pub proposer_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadComment {
    pub id: String,
    pub author_kind: String,
    pub author_name: String,
    pub body: String,
    pub created_at: String,
}

impl Payload {
    /// Event `event` of `report` (as its queue's triage members read it)
    /// in queue `queue_name`, as event `id` at `created_at`; `comment`
    /// the comment it carries (a reply, a reopen's), by ID among the
    /// report's.
    pub fn of(
        id: &str,
        event: Event,
        created_at: &str,
        queue_name: &str,
        report: &Report,
        comment: Option<&str>,
    ) -> Payload {
        let fix = match event {
            Event::FixProposed | Event::Verified | Event::Reopened => {
                report.fixes.last().map(|f| PayloadFix {
                    n: f.n,
                    summary: f.fix.summary.clone(),
                    url: f.fix.url.clone(),
                    commit: f.fix.commit.clone(),
                    release: f.fix.release.clone(),
                    verify_url: f.fix.verify_url.clone(),
                    verify_after: f.fix.verify_after.clone(),
                    outcome: f.outcome,
                    proposer_name: f.proposer_name.clone(),
                })
            }
            _ => None,
        };
        let comment = comment
            .and_then(|cid| report.comments.iter().find(|c| c.id == cid))
            .map(|c| PayloadComment {
                id: c.id.clone(),
                author_kind: c.author_kind.clone(),
                author_name: c.author_name.clone(),
                body: c.body.clone(),
                created_at: c.created_at.clone(),
            });
        Payload {
            id: id.to_string(),
            kind: event.as_str().to_string(),
            created_at: created_at.to_string(),
            queue: PayloadQueue {
                id: report.queue_id.clone(),
                name: queue_name.to_string(),
            },
            report: PayloadReport {
                id: report.id.clone(),
                state: report.state,
                external_id: report.artifact.external_id.clone(),
                artifact_id: report.artifact.id.clone(),
                title: report.artifact.title.clone(),
                page_url: report.artifact.url.clone(),
                link: None,
                reporter_id: report.reporter_id.clone(),
                reporter: report.reporter_profile.clone().filter(|p| !p.is_empty()),
                resolve_reason: report.resolve_reason,
                reopen_count: report.reopen_count,
                created_at: report.created_at.clone(),
            },
            fix,
            comment,
        }
    }
}

/// What the verification delivery ([`VERIFY_EVENT`]) posts: the endpoint
/// it verifies and a random challenge, which [`challenge_answer`] answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyPayload {
    /// The event's ID ([`EVENT_PREFIX`]).
    pub id: String,
    /// Always [`VERIFY_EVENT`].
    #[serde(rename = "type")]
    pub kind: String,
    pub created_at: String,
    pub queue: PayloadQueue,
    pub webhook: PayloadWebhook,
    /// 43 base64url characters (32 random bytes), new for each
    /// verification delivery.
    pub challenge: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadWebhook {
    pub id: String,
    pub url: String,
}

impl VerifyPayload {
    /// A verification of endpoint `webhook_id` at `url` of queue
    /// `queue_id` (`queue_name`), as event `id` at `created_at`, with a
    /// new challenge.
    pub fn new(
        id: &str,
        created_at: &str,
        queue_id: &str,
        queue_name: &str,
        webhook_id: &str,
        url: &str,
    ) -> VerifyPayload {
        VerifyPayload {
            id: id.to_string(),
            kind: VERIFY_EVENT.to_string(),
            created_at: created_at.to_string(),
            queue: PayloadQueue {
                id: queue_id.to_string(),
                name: queue_name.to_string(),
            },
            webhook: PayloadWebhook {
                id: webhook_id.to_string(),
                url: url.to_string(),
            },
            challenge: crate::secret::base64url(&crate::secret::random_bytes::<32>()),
        }
    }
}

/// What an endpoint answers a verification's `challenge` with, as the
/// `answer` of its JSON answer: the hex HMAC-SHA256 under `secret` of
/// `webhook.verify.<challenge>`.
///
/// An HMAC rather than the challenge itself: the request carries the
/// challenge, so a URL that echoes its request (or parts of it, as a
/// JSON echo service does) would hand the challenge back without anyone
/// there holding the secret. The HMAC is in no request, so only a
/// receiver that holds the secret can answer it, whether or not it
/// checks the signature.
pub fn challenge_answer(secret: &str, challenge: &str) -> String {
    let msg = format!("{VERIFY_EVENT}.{challenge}");
    hex(&crate::secret::hmac_sha256(
        secret.as_bytes(),
        msg.as_bytes(),
    ))
}

/// Checks the body of an endpoint's 2xx answer to the verification of
/// `challenge` under `secret`: `whole` (the sender read all of it, at most
/// [`MAX_ANSWER_BYTES`]), a JSON object, and its `answer` a string equal,
/// in constant time, to [`challenge_answer`]. Nothing else in the answer
/// counts: an echo of the request, which holds the challenge, fails.
///
/// # Errors
/// Why the answer verifies nothing, for the delivery log.
pub fn check_answer(secret: &str, challenge: &str, body: &[u8], whole: bool) -> Result<(), String> {
    if !whole || body.len() > MAX_ANSWER_BYTES {
        return Err(format!(
            "the answer is longer than {MAX_ANSWER_BYTES} bytes; answer {{\"answer\": \"<HMAC of the challenge>\"}}"
        ));
    }
    let Ok(serde_json::Value::Object(o)) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Err(
            "the answer is not a JSON object; answer {\"answer\": \"<HMAC of the challenge>\"}"
                .into(),
        );
    };
    let Some(given) = o.get("answer").and_then(|a| a.as_str()) else {
        return Err("the answer has no \"answer\" string".into());
    };
    let want = challenge_answer(secret, challenge);
    if crate::secret::constant_time_eq(given.as_bytes(), want.as_bytes()) {
        Ok(())
    } else {
        Err("the answer is not the HMAC of the challenge under this endpoint's secret".into())
    }
}

/// `s` cut to at most [`MAX_LOGGED_BYTES`] bytes, on a character boundary.
pub fn logged(s: &str) -> String {
    if s.len() <= MAX_LOGGED_BYTES {
        return s.to_string();
    }
    let mut end = MAX_LOGGED_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_and_an_empty_choice_is_every_event() {
        for e in Event::ALL {
            assert_eq!(Event::parse(e.as_str()), Some(*e));
        }
        assert_eq!(parse_events(&[]).unwrap(), Event::ALL.to_vec());
        let got = parse_events(&[
            "report.resolved".into(),
            "report.created".into(),
            "report.created".into(),
        ])
        .unwrap();
        assert_eq!(got, vec![Event::Created, Event::Resolved]);
        let e = parse_events(&["report.deleted".into()]).unwrap_err();
        assert!(
            matches!(
                e,
                CoreError::Invalid {
                    code: "invalid_events",
                    ..
                }
            ),
            "{e:?}"
        );
    }

    /// The signature is HMAC-SHA256 over `timestamp.body`: a fixed vector,
    /// computed independently (`printf '1700000000.{"a":1}' | openssl dgst
    /// -sha256 -hmac whsec_test`).
    #[test]
    fn the_signature_is_hmac_sha256_of_the_timestamp_and_body() {
        assert_eq!(
            signature("whsec_test", 1_700_000_000, br#"{"a":1}"#),
            "v1=38877139021993b830af32feea6e18a8da83eb2f6e49ee50bd9e4cf4ca4d3789"
        );
    }

    #[test]
    fn verify_takes_a_fresh_matching_signature_and_refuses_the_rest() {
        let body = br#"{"id":"evt_1"}"#;
        let sig = signature("whsec_a", 1000, body);
        assert_eq!(
            verify("whsec_a", "1000", &sig, body, 1000 + TOLERANCE_SECS),
            Ok(())
        );
        // Another signature first, as a rotated secret's would be.
        assert_eq!(
            verify("whsec_a", "1000", &format!("v1=00, {sig}"), body, 1000),
            Ok(())
        );
        assert_eq!(
            verify("whsec_b", "1000", &sig, body, 1000),
            Err(VerifyError::BadSignature)
        );
        assert_eq!(
            verify("whsec_a", "1000", &sig, b"{}", 1000),
            Err(VerifyError::BadSignature)
        );
        assert_eq!(
            verify("whsec_a", "1001", &sig, body, 1000),
            Err(VerifyError::BadSignature)
        );
        assert_eq!(
            verify("whsec_a", "1000", &sig, body, 1001 + TOLERANCE_SECS),
            Err(VerifyError::Stale)
        );
        assert_eq!(
            verify("whsec_a", "soon", &sig, body, 1000),
            Err(VerifyError::BadTimestamp)
        );
    }

    #[test]
    fn backoff_grows_fourfold_from_a_minute_to_six_hours() {
        let mins: Vec<u64> = (1..=MAX_ATTEMPTS)
            .map(|n| backoff(n).as_secs() / 60)
            .collect();
        assert_eq!(mins, vec![1, 4, 16, 64, 256, 360, 360, 360]);
        assert_eq!(backoff(0), backoff(1));
        assert_eq!(backoff(u32::MAX).as_secs(), 6 * 3600);
    }

    #[test]
    fn a_local_clax_takes_https_and_loopback_http() {
        for ok in [
            "https://hooks.example.com/clax",
            "https://10.0.0.1/hook",
            "http://localhost:3000/hook",
            "http://127.0.0.1:9/x?y=1",
            "http://[::1]:8080/",
        ] {
            assert!(check_url(ok, false).is_ok(), "{ok}");
        }
        for bad in [
            "http://hooks.example.com/clax",
            "ftp://localhost/",
            "https://user:pw@hooks.example.com/",
            "https://hooks.example.com/#frag",
            "not a url",
        ] {
            assert!(check_url(bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_hosted_clax_takes_only_https_to_a_public_name() {
        assert!(check_url("https://hooks.example.com/clax", true).is_ok());
        for bad in [
            "http://localhost:3000/hook",
            "https://localhost/hook",
            "https://127.0.0.1/hook",
            "https://10.1.2.3/hook",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/hook",
            "https://[fd00::1]/hook",
            "https://2130706433/hook",
            "https://0x7f.0x0.0x0.0x1/hook",
            "https://metadata.google.internal/",
            "https://printer.local/",
            "http://hooks.example.com/clax",
        ] {
            let e = check_url(bad, true).unwrap_err();
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "invalid_url",
                        ..
                    }
                ),
                "{bad}: {e:?}"
            );
        }
    }

    /// The answer to a challenge is HMAC-SHA256 of `webhook.verify.<challenge>`:
    /// a fixed vector, computed independently (`printf
    /// 'webhook.verify.abc' | openssl dgst -sha256 -hmac whsec_test`), the
    /// one the SDK's test checks too.
    #[test]
    fn the_challenge_answer_is_hmac_sha256_of_the_challenge() {
        assert_eq!(
            challenge_answer("whsec_test", "abc"),
            "c9fd1999ad0c5c7fac4ddf087306ac90eb210ef229afe14db35b7b6e4e938344"
        );
    }

    #[test]
    fn only_the_hmac_in_a_small_json_answer_verifies() {
        let answer = challenge_answer("whsec_a", "ch");
        let ok = serde_json::to_vec(&serde_json::json!({ "answer": answer })).unwrap();
        assert_eq!(check_answer("whsec_a", "ch", &ok, true), Ok(()));
        // Read only in part: refused, whatever it starts with.
        assert!(check_answer("whsec_a", "ch", &ok, false).is_err());
        let p = VerifyPayload::new("evt_1", "t", "q", "Q", "w", "https://x.example/");
        let request = serde_json::to_vec(&p).unwrap();
        for bad in [
            // An echo of the request carries the challenge, never its HMAC.
            request.clone(),
            serde_json::to_vec(&serde_json::json!({ "answer": p.challenge })).unwrap(),
            serde_json::to_vec(&serde_json::json!({ "json": p })).unwrap(),
            serde_json::to_vec(&serde_json::json!({ "answer": challenge_answer("whsec_b", "ch") }))
                .unwrap(),
            serde_json::to_vec(&serde_json::json!({ "answer": 1 })).unwrap(),
            serde_json::to_vec(&serde_json::json!([answer])).unwrap(),
            answer.clone().into_bytes(),
            b"".to_vec(),
        ] {
            assert!(
                check_answer("whsec_a", "ch", &bad, true).is_err(),
                "{}",
                String::from_utf8_lossy(&bad)
            );
        }
        let long = format!(
            r#"{{"answer":"{answer}","pad":"{}"}}"#,
            "x".repeat(MAX_ANSWER_BYTES)
        );
        assert!(check_answer("whsec_a", "ch", long.as_bytes(), true).is_err());
    }

    #[test]
    fn a_verification_carries_a_new_challenge_each_time() {
        let a = VerifyPayload::new("evt_1", "t", "q", "Q", "w", "https://x.example/");
        let b = VerifyPayload::new("evt_2", "t", "q", "Q", "w", "https://x.example/");
        assert_eq!(a.kind, VERIFY_EVENT);
        assert_eq!(a.challenge.len(), 43);
        assert_ne!(a.challenge, b.challenge);
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["type"], VERIFY_EVENT);
        assert_eq!(v["webhook"]["url"], "https://x.example/");
    }

    #[test]
    fn logged_text_is_cut_on_a_character_boundary() {
        let s = "é".repeat(MAX_LOGGED_BYTES);
        let cut = logged(&s);
        assert!(cut.len() <= MAX_LOGGED_BYTES + "…".len());
        assert!(cut.ends_with('…'));
        assert_eq!(logged("short"), "short");
    }
}

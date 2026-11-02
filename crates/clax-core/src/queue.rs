//! Report queues and external artifacts (spec 2026-10-27-clax-sdk-design
//! §6, §7; spec 2026-10-29-feedback-loop-design §6): the records a
//! queue's routes serve, their limits, and the checks of what a deployer's
//! app sends. The store keeps them (`clax_store::queues`,
//! `clax_store::external`).

use crate::anchor::Anchor;
use crate::report::{
    DiagLine, FixAttempt, ReportContext, ReporterProfile, ResolveReason, ServerStamp,
};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// How long a claim on a report lasts unless renewed, in seconds (§7.4).
pub const LEASE_SECS: i64 = 600;
/// How long a reporter token lasts, in seconds (§5.3).
pub const REPORTER_TOKEN_SECS: i64 = 900;
/// The longest `external_id`, in bytes (§6.1).
pub const MAX_EXTERNAL_ID_BYTES: usize = 512;
/// The largest artifact `metadata`, as compact JSON, in bytes (§6.1).
pub const MAX_ARTIFACT_METADATA_BYTES: usize = 64 * 1024;
/// The largest thread `metadata`, as compact JSON, in bytes (§6.4).
pub const MAX_THREAD_METADATA_BYTES: usize = 16 * 1024;
/// The longest queue name, in characters.
pub const MAX_QUEUE_NAME_CHARS: usize = 120;
/// The most origins a queue lists.
pub const MAX_QUEUE_ORIGINS: usize = 32;
/// The longest reporter ID, in bytes.
pub const MAX_REPORTER_ID_BYTES: usize = 256;
/// The longest revision, in bytes.
pub const MAX_REVISION_BYTES: usize = 256;
/// Reports per page of a listing.
pub const REPORT_PAGE: usize = 50;
/// The prefix of a service grant's token.
pub const SERVICE_TOKEN_PREFIX: &str = "cxq_";
/// The prefix of a reporter token.
pub const REPORTER_TOKEN_PREFIX: &str = "cxr_";
/// A comment's `author_kind` in a report view when a reporter wrote it.
pub const AUTHOR_REPORTER: &str = "reporter";

/// Whose reports a reporter token reads (§7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReportersSee {
    /// Only its own reporter's (the default, D8).
    Own,
    /// Every report in the queue.
    All,
}

impl ReportersSee {
    /// The stored and served name.
    pub fn as_str(self) -> &'static str {
        match self {
            ReportersSee::Own => "own",
            ReportersSee::All => "all",
        }
    }

    /// The value named `s`.
    pub fn parse(s: &str) -> Option<ReportersSee> {
        match s {
            "own" => Some(ReportersSee::Own),
            "all" => Some(ReportersSee::All),
            _ => None,
        }
    }
}

/// A member's right on a queue (§7.1): `admin` changes the queue and makes
/// service grants, and holds triage too; `triage` reads every report,
/// claims, replies, resolves and writes thread metadata.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[schemars(rename = "QueueRole")]
pub enum Role {
    Triage,
    Admin,
}

impl Role {
    /// The stored and served name.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Triage => "triage",
        }
    }

    /// The role named `s`.
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "admin" => Some(Role::Admin),
            "triage" => Some(Role::Triage),
            _ => None,
        }
    }
}

/// Where a report stands (spec 2026-10-29-feedback-loop-design §6.1):
/// `open`, waiting for someone; `claimed`, while a live lease holds it;
/// `fix_proposed`, while a fix waits for the reporter's verdict;
/// `verified`, once the reporter said it is fixed; `resolved`, once triage
/// closed it without a verdict. `verified` and `resolved` resolve the
/// report's thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ReportState")]
pub enum State {
    Open,
    Claimed,
    FixProposed,
    Verified,
    Resolved,
}

impl State {
    /// Every state, in the order a report moves through them.
    pub const ALL: &'static [State] = &[
        State::Open,
        State::Claimed,
        State::FixProposed,
        State::Verified,
        State::Resolved,
    ];

    /// The stored and served name.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Claimed => "claimed",
            State::FixProposed => "fix_proposed",
            State::Verified => "verified",
            State::Resolved => "resolved",
        }
    }

    /// The state named `s`.
    pub fn parse(s: &str) -> Option<State> {
        State::ALL.iter().copied().find(|st| st.as_str() == s)
    }

    /// Whether a report in this state has its thread resolved.
    pub fn is_closed(self) -> bool {
        matches!(self, State::Verified | State::Resolved)
    }
}

/// The order a listing of a queue's reports pages in: by when each was
/// filed, `oldest` first (the default) or `newest` first; reports filed in
/// the same instant are ordered by ID the same way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportOrder {
    #[default]
    Oldest,
    Newest,
}

impl ReportOrder {
    /// The order's name in a query string.
    pub fn as_str(self) -> &'static str {
        match self {
            ReportOrder::Oldest => "oldest",
            ReportOrder::Newest => "newest",
        }
    }

    /// The order named `s`.
    pub fn parse(s: &str) -> Option<ReportOrder> {
        match s {
            "oldest" => Some(ReportOrder::Oldest),
            "newest" => Some(ReportOrder::Newest),
            _ => None,
        }
    }
}

/// Which reports a listing returns (§6.1): those in one state, or the
/// `active` ones, `open` or `claimed`. Serialized as its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateFilter {
    One(State),
    Active,
}

impl StateFilter {
    /// The filter's name in a query string: a state's name, or `active`.
    pub fn as_str(self) -> &'static str {
        match self {
            StateFilter::One(s) => s.as_str(),
            StateFilter::Active => "active",
        }
    }

    /// The filter named `s`.
    pub fn parse(s: &str) -> Option<StateFilter> {
        match s {
            "active" => Some(StateFilter::Active),
            _ => State::parse(s).map(StateFilter::One),
        }
    }

    /// Whether a report in `state` passes the filter.
    pub fn matches(self, state: State) -> bool {
        match self {
            StateFilter::One(s) => s == state,
            StateFilter::Active => matches!(state, State::Open | State::Claimed),
        }
    }
}

impl Serialize for StateFilter {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for StateFilter {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        StateFilter::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{s:?} is not a report state (open, claimed, fix_proposed, verified, resolved) or active"
            ))
        })
    }
}

/// A queue as a member reads it, with the reader's own role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Queue {
    pub id: String,
    pub name: String,
    /// The deployer's app's origins (normalized by [`normalize_origin`]):
    /// a reporter token is minted only for one of them.
    pub origins: Vec<String>,
    pub reporters_see: ReportersSee,
    pub created_at: String,
    pub role: Role,
}

/// The external artifact a report is on, as a report carries it (§6.1).
/// Its fields are shared by every queue whose reports it holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReportArtifact {
    pub id: String,
    pub external_id: String,
    pub title: String,
    pub url: String,
    /// Opaque JSON the app wrote; `null` when it wrote none.
    pub metadata: serde_json::Value,
    pub metadata_version: u64,
    pub revision: Option<String>,
}

/// A comment of a report: `author_kind` is `viewer`, `agent` or
/// [`AUTHOR_REPORTER`]. A reopen's comment (feedback-loop §6.3) carries
/// its capture: the reopen's number, whether a clip came with it, and how
/// many diagnostics lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReportComment {
    pub id: String,
    #[schemars(extend("enum" = ["viewer", "agent", "reporter"]))]
    pub author_kind: String,
    pub author_name: String,
    pub body: String,
    pub created_at: String,
    /// The reopen this comment was posted with (1 for the first); absent
    /// on any other comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_clip: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub diagnostics_lines: u32,
    /// A note for the queue's members alone (a person's release of a
    /// claim): a reporter's view of the report leaves it out.
    #[serde(default, skip_serializing_if = "is_false")]
    pub triage_only: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A live claim on a report: who holds it, until when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ReportClaim")]
pub struct Claim {
    pub holder_name: String,
    pub expires_at: String,
}

/// A report: a thread filed through a queue (§7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Report {
    /// The thread's ID.
    pub id: String,
    pub queue_id: String,
    pub artifact: ReportArtifact,
    pub anchor: Anchor,
    /// The thread's status: `open` or `resolved`.
    #[schemars(extend("enum" = ["open", "resolved"]))]
    pub status: String,
    pub state: State,
    /// The reporter that filed it, from its reporter token; `None` for a
    /// report filed otherwise.
    pub reporter_id: Option<String>,
    /// Opaque JSON the app wrote with the report; `null` when none.
    pub metadata: serde_json::Value,
    pub metadata_version: u64,
    pub has_clip: bool,
    pub comments: Vec<ReportComment>,
    pub claim: Option<Claim>,
    pub created_at: String,
    /// What the reporter's browser said at capture (feedback-loop §5.1);
    /// absent on a report filed without one, and on a report served to a
    /// reporter that did not file it (`reporters_see: all`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ReportContext>,
    /// The receiving server's stamp (feedback-loop §5.1); absent on a
    /// report filed without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerStamp>,
    /// The reporter's profile (feedback-loop §5.4), served to triage
    /// members only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporter_profile: Option<ReporterProfile>,
    /// Why a `verified` or `resolved` report was closed (feedback-loop
    /// §6.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve_reason: Option<ResolveReason>,
    /// The fixes proposed on it, oldest first (feedback-loop §6.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<FixAttempt>,
    /// How many times the reporter (or triage on their behalf) reopened
    /// it (feedback-loop §6.3).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reopen_count: u32,
    /// How many diagnostics lines its latest capture holding any has: the
    /// filing's, or a later reopen's (feedback-loop §5.2); 0 when none
    /// has.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub diagnostics_lines: u32,
    /// How many diagnostics lines the filing itself (capture 0) has; 0
    /// when it came with none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub filing_diagnostics_lines: u32,
}

impl Report {
    /// The report as a reporter-facing route serializes it: whatever the
    /// store answered, never a comment for the queue's members alone
    /// (`triage_only`) nor the reporter's profile (feedback-loop §5.4).
    /// The store's reporter view leaves them out already; this holds at
    /// the serving layer too.
    #[must_use]
    pub fn for_reporter(mut self) -> Self {
        self.comments.retain(|c| !c.triage_only);
        self.reporter_profile = None;
        self
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A claim's answer (§7.4): the lease, its expiry, its fence (larger for
/// each new lease on the report), and the report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Lease {
    pub lease_id: String,
    pub expires_at: String,
    pub fence: u64,
    pub report: Report,
}

// The bodies of the queue routes in the API contract (`crate::contract`):
// the handlers read and answer these types, so the contract's schemas are
// theirs.

/// `{report}`: the answer of a route that changes or files one report.
#[derive(Clone, Debug, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ReportAnswer {
    pub report: Report,
}

/// A page of reports; `next` is the cursor of the following page, `null`
/// on the last.
#[derive(Clone, Debug, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ReportPage {
    pub reports: Vec<Report>,
    pub next: Option<String>,
}

/// `{lines}`: a capture's diagnostics, `[]` when it has none.
#[derive(Clone, Debug, PartialEq, Serialize, schemars::JsonSchema)]
pub struct DiagnosticsAnswer {
    pub lines: Vec<DiagLine>,
}

/// What waits for a reporter: its reports with a fix waiting for its
/// verdict, or with a reply it has not read (`…/seen`), oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Badge {
    /// How many reports wait (`report_ids.len()`).
    pub waiting: u32,
    pub report_ids: Vec<String>,
}

/// `{queues}`: the caller's queues with its role on each.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct QueueList {
    pub queues: Vec<Queue>,
}

/// A service grant as its queue's admins see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ServiceGrantInfo {
    pub id: String,
    pub name: String,
    pub created_at: String,
}

/// A new service grant: its token (`cxq_…`) and its Ed25519 key, shown in
/// this answer alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ServiceGrantCreated {
    pub grant: ServiceGrantInfo,
    pub token: String,
    /// The key's PKCS#8 v2 document, unpadded base64url.
    pub private_key: String,
}

/// A service grant's request for a reporter token.
#[derive(Clone, Debug, PartialEq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReporterTokenRequest {
    pub reporter_id: String,
    /// One of the queue's origins.
    pub origin: String,
    /// The reporter key's 32 public bytes in unpadded base64url.
    pub public_key: String,
    /// Who the backend says the reporter is; each report filed under the
    /// token keeps it. Read as JSON first, so a bad one is 400
    /// `invalid_profile` rather than `invalid_args`.
    #[serde(default)]
    #[schemars(with = "Option<ReporterProfile>")]
    pub profile: Option<serde_json::Value>,
}

/// A reporter token (`cxr_…`), bound to a key, an origin and a reporter ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ReporterToken {
    pub token: String,
    pub expires_at: String,
}

/// A comment's body.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentRequest {
    pub body: String,
}

/// A reporter's verdict that a fix works: a note, and its browser's
/// context at the verdict (checked and scrubbed as a filing's). An empty
/// body is `{}`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyRequest {
    #[serde(default)]
    pub note: Option<String>,
    /// Read as JSON first, so a bad one is 400 `invalid_context`.
    #[serde(default)]
    #[schemars(with = "Option<ReportContext>")]
    pub context: Option<serde_json::Value>,
}

/// A listing's query: the cursor a page's `next` gave.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    #[serde(default)]
    pub cursor: Option<String>,
}

/// A diagnostics query: capture `capture`'s (0 the filing's, `n` the
/// `n`th reopen's), or without it the latest capture's that has any.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureQuery {
    #[serde(default)]
    pub capture: Option<u32>,
}

/// `Ok` for an `external_id` Clax keeps: 1 to [`MAX_EXTERNAL_ID_BYTES`]
/// bytes, compared byte for byte and never parsed.
///
/// # Errors
/// `invalid_external_id` otherwise.
pub fn check_external_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > MAX_EXTERNAL_ID_BYTES {
        return Err(CoreError::invalid(
            "invalid_external_id",
            format!("an external ID is 1 to {MAX_EXTERNAL_ID_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// `value` as Clax stores it (compact JSON), when it is at most `max`
/// bytes so.
///
/// # Errors
/// `metadata_too_large` otherwise.
pub fn metadata_json(value: &serde_json::Value, max: usize) -> Result<String> {
    let s = serde_json::to_string(value).expect("JSON values serialise");
    if s.len() > max {
        return Err(CoreError::invalid(
            "metadata_too_large",
            format!("metadata is at most {max} bytes as JSON"),
        ));
    }
    Ok(s)
}

/// The SHA-256 of a stored metadata value (its compact JSON), as the
/// journal records it: `sha256:<hex>`.
pub fn metadata_sha256(json: &str) -> String {
    format!("sha256:{}", crate::audit::sha256_hex(json.as_bytes()))
}

/// `raw` as a queue lists an origin: `scheme://host[:port]` of an `http`
/// or `https` URL with nothing after it, the host lowercased and a default
/// port left out, as a browser's `Origin` header names it.
///
/// # Errors
/// `invalid_origin` for anything else.
pub fn normalize_origin(raw: &str) -> Result<String> {
    let bad = || {
        CoreError::invalid(
            "invalid_origin",
            format!("{raw:?} is not an origin (scheme://host[:port])"),
        )
    };
    let u = url::Url::parse(raw.trim()).map_err(|_| bad())?;
    if !matches!(u.scheme(), "http" | "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.path() != "/"
    {
        return Err(bad());
    }
    match u.origin() {
        url::Origin::Tuple(..) => Ok(u.origin().ascii_serialization()),
        url::Origin::Opaque(_) => Err(bad()),
    }
}

/// `name` trimmed, when it is a queue name: 1 to [`MAX_QUEUE_NAME_CHARS`]
/// characters.
///
/// # Errors
/// `invalid_name` otherwise.
pub fn check_queue_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_QUEUE_NAME_CHARS {
        return Err(CoreError::invalid(
            "invalid_name",
            format!("a queue name is 1 to {MAX_QUEUE_NAME_CHARS} characters"),
        ));
    }
    Ok(n.to_string())
}

/// `Ok` for a reporter ID: 1 to [`MAX_REPORTER_ID_BYTES`] bytes, opaque.
///
/// # Errors
/// `invalid_reporter_id` otherwise.
pub fn check_reporter_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > MAX_REPORTER_ID_BYTES {
        return Err(CoreError::invalid(
            "invalid_reporter_id",
            format!("a reporter ID is 1 to {MAX_REPORTER_ID_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// `Ok` for a revision: at most [`MAX_REVISION_BYTES`] bytes, opaque.
///
/// # Errors
/// `invalid_revision` otherwise.
pub fn check_revision(rev: &str) -> Result<()> {
    if rev.len() > MAX_REVISION_BYTES {
        return Err(CoreError::invalid(
            "invalid_revision",
            format!("a revision is at most {MAX_REVISION_BYTES} bytes"),
        ));
    }
    Ok(())
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Version 1 of an external artifact: Clax keeps no content of it (§6.1),
/// so its one page links to the record at `url`, following the page
/// contract.
pub fn external_placeholder_html(title: &str, url: &str) -> String {
    let t = escape_html(title);
    let u = escape_html(url);
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{t}</title><style>:root{{color-scheme:light dark;--bg:#f7f7f5;--ink:#1c1b19;--muted:#6b6862}}@media (prefers-color-scheme:dark){{:root{{--bg:#141413;--ink:#ecebe7;--muted:#a8a59e}}}}body{{margin:0;padding:48px 16px;background:var(--bg);color:var(--ink);font:16px/1.5 ui-sans-serif,-apple-system,system-ui,sans-serif}}main{{max-width:40rem;margin:0 auto}}p{{color:var(--muted)}}a{{color:inherit}}</style></head><body><main><h1>{t}</h1><p>This artifact lives in another app. Open <a href=\"{u}\">{u}</a> to see it.</p></main></body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_normalize_as_a_browser_names_them() {
        assert_eq!(
            normalize_origin("https://App.Example.com").unwrap(),
            "https://app.example.com"
        );
        assert_eq!(
            normalize_origin("https://app.example.com:443/").unwrap(),
            "https://app.example.com"
        );
        assert_eq!(
            normalize_origin("http://localhost:5173").unwrap(),
            "http://localhost:5173"
        );
        for bad in [
            "app.example.com",
            "ftp://x.example",
            "https://x.example/path",
            "https://x.example/?q=1",
            "https://u@x.example",
            "https://x.example//",
        ] {
            assert!(normalize_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn limits_bound_ids_and_metadata() {
        assert!(check_external_id("").is_err());
        assert!(check_external_id(&"x".repeat(MAX_EXTERNAL_ID_BYTES)).is_ok());
        assert!(check_external_id(&"x".repeat(MAX_EXTERNAL_ID_BYTES + 1)).is_err());
        let big = serde_json::json!("x".repeat(MAX_THREAD_METADATA_BYTES));
        assert!(metadata_json(&big, MAX_THREAD_METADATA_BYTES).is_err());
        assert_eq!(
            metadata_json(&serde_json::json!({"a": 1}), 16).unwrap(),
            "{\"a\":1}"
        );
    }

    #[test]
    fn names_round_trip() {
        for s in [ReportersSee::Own, ReportersSee::All] {
            assert_eq!(ReportersSee::parse(s.as_str()), Some(s));
        }
        for r in [Role::Admin, Role::Triage] {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        for s in [State::Open, State::Claimed, State::Resolved] {
            assert_eq!(State::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_str());
        }
        assert!(Role::Admin > Role::Triage);
    }

    #[test]
    fn states_round_trip_including_fix_proposed_and_verified() {
        for s in [
            State::Open,
            State::Claimed,
            State::FixProposed,
            State::Verified,
            State::Resolved,
        ] {
            assert_eq!(State::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_str());
            assert_eq!(
                serde_json::from_value::<State>(s.as_str().into()).unwrap(),
                s
            );
        }
        assert_eq!(State::FixProposed.as_str(), "fix_proposed");
        assert_eq!(State::Verified.as_str(), "verified");
        assert_eq!(State::parse("active"), None);
    }

    #[test]
    fn active_filter_parses() {
        assert_eq!(StateFilter::parse("active"), Some(StateFilter::Active));
        assert_eq!(
            StateFilter::parse("fix_proposed"),
            Some(StateFilter::One(State::FixProposed))
        );
        assert_eq!(StateFilter::parse("closed"), None);
        for f in [StateFilter::Active, StateFilter::One(State::Verified)] {
            assert_eq!(StateFilter::parse(f.as_str()), Some(f));
        }
        assert!(StateFilter::Active.matches(State::Open));
        assert!(StateFilter::Active.matches(State::Claimed));
        for s in [State::FixProposed, State::Verified, State::Resolved] {
            assert!(!StateFilter::Active.matches(s));
        }
        assert!(StateFilter::One(State::Verified).matches(State::Verified));
        assert!(!StateFilter::One(State::Verified).matches(State::Resolved));
    }

    /// A milestone-1 report (no context, fixes or counts) serializes as it
    /// did, and reads back.
    #[test]
    fn report_fields_of_the_feedback_loop_are_additive() {
        let anchor: Anchor =
            serde_json::from_value(serde_json::json!({"kind": "artifact", "file": ""})).unwrap();
        let m1 = serde_json::json!({
            "id": "t1", "queue_id": "q1",
            "artifact": {"id": "a1", "external_id": "e", "title": "T", "url": "https://x.example/",
                         "metadata": null, "metadata_version": 0, "revision": null},
            "anchor": anchor,
            "status": "open", "state": "open", "reporter_id": null, "metadata": null,
            "metadata_version": 0, "has_clip": false, "comments": [], "claim": null,
            "created_at": "2026-10-29T07:12:03.120Z"
        });
        let r: Report = serde_json::from_value(m1.clone()).unwrap();
        assert!(r.context.is_none() && r.fixes.is_empty());
        assert_eq!((r.reopen_count, r.diagnostics_lines), (0, 0));
        assert_eq!(serde_json::to_value(&r).unwrap(), m1);
    }

    /// A reporter-facing serialization never carries a triage-only note or
    /// the reporter's profile, even from a report that holds them.
    #[test]
    fn a_reporters_report_never_carries_a_triage_only_note() {
        let anchor: Anchor =
            serde_json::from_value(serde_json::json!({"kind": "artifact", "file": ""})).unwrap();
        let comment = |id: &str, triage_only: bool| ReportComment {
            id: id.into(),
            author_kind: "viewer".into(),
            author_name: "Ana".into(),
            body: format!("note {id}"),
            created_at: "2026-10-29T07:12:03.120Z".into(),
            capture: None,
            has_clip: false,
            diagnostics_lines: 0,
            triage_only,
        };
        let r = Report {
            id: "t1".into(),
            queue_id: "q1".into(),
            artifact: ReportArtifact {
                id: "a1".into(),
                external_id: "e".into(),
                title: "T".into(),
                url: "https://x.example/".into(),
                metadata: serde_json::Value::Null,
                metadata_version: 0,
                revision: None,
            },
            anchor,
            status: "open".into(),
            state: State::Open,
            reporter_id: Some("emp-1".into()),
            metadata: serde_json::Value::Null,
            metadata_version: 0,
            has_clip: false,
            comments: vec![comment("c1", false), comment("c2", true)],
            claim: None,
            created_at: "2026-10-29T07:12:03.120Z".into(),
            context: None,
            server: None,
            reporter_profile: Some(ReporterProfile {
                name: Some("Ana".into()),
                ..Default::default()
            }),
            resolve_reason: None,
            fixes: Vec::new(),
            reopen_count: 0,
            diagnostics_lines: 0,
            filing_diagnostics_lines: 0,
        };
        let v = serde_json::to_value(r.for_reporter()).unwrap();
        let ids: Vec<&str> = v["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["c1"], "{v}");
        assert!(!v.to_string().contains("triage_only"), "{v}");
        assert!(v.get("reporter_profile").is_none(), "{v}");
    }
}

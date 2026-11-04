//! Report queues and external artifacts (spec 2026-10-27-clax-sdk-design
//! §6, §7; spec 2026-10-29-feedback-loop-design §6): the records a
//! queue's routes serve, their limits, and the checks of what a deployer's
//! app sends. The store keeps them (`clax_store::queues`,
//! `clax_store::external`).

use crate::anchor::Anchor;
use crate::report::{FixAttempt, ReportContext, ReporterProfile, ResolveReason, ServerStamp};
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    pub id: String,
    pub name: String,
    /// The deployer's app's origins and origin patterns (normalized by
    /// [`normalize_queue_origin`]): a reporter token is minted only for an
    /// exact origin they allow ([`queue_allows_origin`]).
    pub origins: Vec<String>,
    pub reporters_see: ReportersSee,
    pub created_at: String,
    pub role: Role,
}

/// The external artifact a report is on, as a report carries it (§6.1).
/// Its fields are shared by every queue whose reports it holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportComment {
    pub id: String,
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub holder_name: String,
    pub expires_at: String,
}

/// A report: a thread filed through a queue (§7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The thread's ID.
    pub id: String,
    pub queue_id: String,
    pub artifact: ReportArtifact,
    pub anchor: Anchor,
    /// The thread's status: `open` or `resolved`.
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    pub lease_id: String,
    pub expires_at: String,
    pub fence: u64,
    pub report: Report,
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

/// `raw` as a browser's `Origin` header names it: `scheme://host[:port]`
/// of an `http` or `https` URL with nothing after it, the host lowercased
/// and a default port left out. A reporter token is bound to one such
/// exact origin.
///
/// # Errors
/// `invalid_origin` for anything else, an origin pattern (holding a `*`)
/// included.
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
        url::Origin::Tuple(..) => {
            let o = u.origin().ascii_serialization();
            if o.contains('*') {
                return Err(bad());
            }
            Ok(o)
        }
        url::Origin::Opaque(_) => Err(bad()),
    }
}

/// The longest host an origin pattern names, and the longest it matches,
/// in bytes: a DNS name's, so at most 127 labels.
const MAX_PATTERN_HOST_BYTES: usize = 253;

/// `raw` as a queue lists it: an exact origin ([`normalize_origin`]), or,
/// when it holds a `*`, an origin pattern normalized the same way (the
/// host lowercased, a default port left out, nothing after
/// `host[:port]`).
///
/// A pattern's scheme (`http` or `https`) and port are exact, never a
/// `*`. Its host, at most 253 bytes, is ASCII (an internationalized label
/// written as its `xn--` form), of labels of `a`-`z`, `0`-`9`, `-` and
/// `*`, each holding at most one `*`:
///
/// - a label that is only `*` stands for one or more whole labels, at
///   any place in the host;
/// - a label holding a `*` beside other characters (`pr-*`, `*-eu`)
///   stands for that label with one or more of `a`-`z`, `0`-`9` and `-`
///   in place of the `*`, never a `.`.
///
/// At least two labels without a `*` follow the last label holding one.
/// So `https://*.example.com`, `https://a.*.example.com` and
/// `https://app-pr-*.example.com` are patterns, and `https://*`,
/// `https://*.com`, `https://pr-*.com` and `https://a.*.com` are not.
///
/// # Errors
/// `invalid_origin` for anything else.
pub fn normalize_queue_origin(raw: &str) -> Result<String> {
    if !raw.contains('*') {
        return normalize_origin(raw);
    }
    let (scheme, host, port) = pattern_parts(raw.trim()).ok_or_else(|| {
        CoreError::invalid(
            "invalid_origin",
            format!(
                "{raw:?} is not an origin pattern: scheme://host[:port] with a * only in the \
                 host, as a whole label (https://*.example.com) or part of one \
                 (https://app-pr-*.example.com), followed by at least two labels without one"
            ),
        )
    })?;
    Ok(match port {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
}

/// The scheme, lowercased host and port (`None` for the scheme's default)
/// of origin pattern `s`, when it is one ([`normalize_queue_origin`]).
fn pattern_parts(s: &str) -> Option<(&'static str, String, Option<u16>)> {
    let (scheme, rest) = s.split_once("://")?;
    let (scheme, default_port) = match scheme.to_ascii_lowercase().as_str() {
        "http" => ("http", 80),
        "https" => ("https", 443),
        _ => return None,
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (h, Some(p.parse::<u16>().ok()?))
        }
        None => (rest, None),
    };
    let host = host.to_ascii_lowercase();
    if host.len() > MAX_PATTERN_HOST_BYTES {
        return None;
    }
    let mut wild = false;
    let mut tail = 0;
    for label in host.split('.') {
        let stars = label.bytes().filter(|&b| b == b'*').count();
        if label.is_empty()
            || label.len() > 63
            || stars > 1
            || !label.bytes().all(|b| label_byte(b) || b == b'*')
        {
            return None;
        }
        if stars == 1 {
            wild = true;
            tail = 0;
        } else {
            tail += 1;
        }
    }
    if !wild || tail < 2 {
        return None;
    }
    Some((scheme, host, port.filter(|&p| p != default_port)))
}

/// Whether `b` may stand in a pattern's label, and in what its `*` matches.
fn label_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
}

/// Whether a queue listing `listed` (each normalized by
/// [`normalize_queue_origin`]) allows exact origin `origin`: `origin` is
/// one of its exact origins, or, written as [`normalize_origin`] writes
/// it with a DNS name for its host (never an IP address), matched by one
/// of its patterns: the same scheme and port, and a host whose labels,
/// each of `a`-`z`, `0`-`9` and `-`, are the pattern's, with one or more
/// whole labels in place of each `*` label and one or more of those
/// characters in place of each other `*`. A pattern is never compared
/// as text: `origin` equal to a pattern is not allowed by it.
pub fn queue_allows_origin(listed: &[String], origin: &str) -> bool {
    listed.iter().any(|l| {
        if l.contains('*') {
            pattern_matches(l, origin)
        } else {
            l == origin
        }
    })
}

/// Whether origin pattern `pattern` matches `origin`, an exact origin as
/// [`normalize_origin`] writes it.
fn pattern_matches(pattern: &str, origin: &str) -> bool {
    let Some((scheme, host, port)) = pattern_parts(pattern) else {
        return false;
    };
    if normalize_origin(origin).ok().as_deref() != Some(origin) {
        return false;
    }
    let Ok(u) = url::Url::parse(origin) else {
        return false;
    };
    let Some(url::Host::Domain(o_host)) = u.host() else {
        return false;
    };
    if o_host.len() > MAX_PATTERN_HOST_BYTES {
        return false;
    }
    let o_labels: Vec<&str> = o_host.split('.').collect();
    if !o_labels
        .iter()
        .all(|l| !l.is_empty() && l.bytes().all(label_byte))
    {
        return false;
    }
    let p_labels: Vec<&str> = host.split('.').collect();
    u.scheme() == scheme && u.port() == port && labels_match(&p_labels, &o_labels)
}

/// Whether host labels `o` are pattern labels `p`: each `*` label of `p`
/// in place of one or more labels of `o`, and each other label of `p` in
/// place of one ([`label_matches`]).
///
/// Greedy, without recursion: a `*` label first takes one label, and
/// when what follows fails, the last `*` met takes one label more and
/// the labels after it are tried again. The labels between two `*`s each
/// take exactly one, so the first place they fit is never worse than a
/// later one, and only the last `*` ever needs to grow. Each retry grows
/// it by one label, so the work is at most `p.len()` label comparisons
/// per label of `o`: 127 × 127 for the longest hosts
/// ([`MAX_PATTERN_HOST_BYTES`]).
fn labels_match(p: &[&str], o: &[&str]) -> bool {
    let (mut i, mut j) = (0, 0);
    // The pattern label after the last `*` met, and the label of `o`
    // after the labels it takes.
    let mut star: Option<(usize, usize)> = None;
    while j < o.len() {
        match p.get(i) {
            Some(&"*") => {
                i += 1;
                j += 1;
                star = Some((i, j));
            }
            Some(l) if label_matches(l, o[j]) => {
                i += 1;
                j += 1;
            }
            _ => match star {
                Some((after, taken)) => {
                    i = after;
                    j = taken + 1;
                    star = Some((after, j));
                }
                None => return false,
            },
        }
    }
    i == p.len()
}

/// Whether label `o` is pattern label `p`, not a `*` label: equal to it,
/// or, when `p` holds a `*`, its text before and after the `*` around one
/// or more [`label_byte`]s.
fn label_matches(p: &str, o: &str) -> bool {
    match p.split_once('*') {
        None => p == o,
        Some((before, after)) => {
            o.len() > before.len() + after.len()
                && o.starts_with(before)
                && o.ends_with(after)
                && o.as_bytes()[before.len()..o.len() - after.len()]
                    .iter()
                    .all(|&b| label_byte(b))
        }
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
    fn an_exact_origin_holds_no_star() {
        for bad in [
            "https://pr-*.fly.dev",
            "https://*.fly.dev",
            "https://pr-%2A.fly.dev",
        ] {
            assert!(normalize_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn patterns_normalize_as_exact_origins_do() {
        let n = |raw| normalize_queue_origin(raw).unwrap();
        assert_eq!(
            n("https://pathbase-pr-*.fly.dev"),
            "https://pathbase-pr-*.fly.dev"
        );
        assert_eq!(
            n(" HTTPS://PathBase-PR-*.Fly.Dev/ "),
            "https://pathbase-pr-*.fly.dev"
        );
        assert_eq!(n("HTTPS://*.Fly.Dev:443/"), "https://*.fly.dev");
        assert_eq!(n("https://pr-*.fly.dev:443"), "https://pr-*.fly.dev");
        assert_eq!(n("http://*.fly.dev:80/"), "http://*.fly.dev");
        assert_eq!(n("https://pr-*.fly.dev:8443"), "https://pr-*.fly.dev:8443");
        assert_eq!(n("http://*.fly.dev:443"), "http://*.fly.dev:443");
        assert_eq!(n("https://*-eu.example.com"), "https://*-eu.example.com");
        assert_eq!(n("https://a*b.example.com"), "https://a*b.example.com");
        assert_eq!(
            n("https://web.pr-*.xn--bcher-kva.example"),
            "https://web.pr-*.xn--bcher-kva.example"
        );
        // Wildcard labels anywhere before the last two, any number of them.
        for kept in [
            "https://*.fly.dev",
            "https://a.*.example.com",
            "https://*.*.example.com",
            "https://*.a.*.example.com",
            "https://*.api-*.example.com",
            "https://a-*.b-*.example.com",
        ] {
            assert_eq!(n(kept), kept);
        }
        // An exact origin is normalized as before.
        assert_eq!(n("https://App.Example.com:443/"), "https://app.example.com");
    }

    #[test]
    fn a_bare_star_host_or_a_one_label_tail_is_refused_and_so_is_a_malformed_pattern() {
        for bad in [
            // A bare `*` host.
            "https://*",
            "http://*:8080",
            // One label, or none, after the last label holding a `*`.
            "https://*.dev",
            "https://pr-*.com",
            "https://a.*.com",
            "https://*.*",
            "https://pr-*.fly-*.dev",
            "https://*.example.*",
            "https://app.pr-*",
            "https://example.com-*",
            // Two `*`s in one label.
            "https://**.example.com",
            "https://a**.example.com",
            "https://a*b*.example.com",
            "https://*a*.example.com",
            // A `*` outside the host.
            "http*://pr-*.example.com",
            "*://pr-*.example.com",
            "https://pr-*.example.com:*",
            "https://pr-*.example.com:84*",
            "https://pr-*.example.com/*",
            "https://example.com:*",
            "https://example.com/*",
            // Anything after host[:port], userinfo.
            "https://pr-*.example.com/path",
            "https://pr-*.example.com?q=1",
            "https://pr-*.example.com#f",
            "https://u@pr-*.example.com",
            "https://pr-*.example.com//",
            // Another scheme, no scheme, a bad port.
            "ftp://*.example.com",
            "*.example.com",
            "https://pr-*.example.com:",
            "https://pr-*.example.com:65536",
            // Characters a DNS label does not hold, non-ASCII, empty labels.
            "https://pr_*.example.com",
            "https://pr-*.bücher.example",
            "https://*..example.com",
            "https://.*.example.com",
            "https://*.example.com.",
            "https://[*::1]",
        ] {
            let e = normalize_queue_origin(bad).expect_err(bad);
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "invalid_origin",
                        ..
                    }
                ),
                "{bad}: {e:?}"
            );
        }
        let long = format!("https://{}*.example.com", "a".repeat(63));
        assert!(normalize_queue_origin(&long).is_err());
        let longest = format!("https://{}*.example.com", "a".repeat(62));
        assert!(normalize_queue_origin(&longest).is_ok());
        // 253 bytes of host at most.
        let host = |labels: usize| format!("https://{}example.com", "*.".repeat(labels));
        assert_eq!(host(121).len() - "https://".len(), 253);
        assert!(normalize_queue_origin(&host(121)).is_ok());
        assert!(normalize_queue_origin(&host(122)).is_err());
    }

    #[test]
    fn a_partial_star_matches_within_its_label_and_nothing_else() {
        let listed = vec![normalize_queue_origin("https://pathbase-pr-*.fly.dev").unwrap()];
        let allows = |o: &str| queue_allows_origin(&listed, o);
        assert!(allows("https://pathbase-pr-538.fly.dev"));
        assert!(allows("https://pathbase-pr-1.fly.dev"));
        assert!(allows("https://pathbase-pr-a-b.fly.dev"));
        assert!(allows("https://pathbase-pr-12ab.fly.dev"));
        for not in [
            // Another label, another domain, an empty match.
            "https://pathbase-pr-1.evil.fly.dev",
            "https://pathbase-pr-1.fly.dev.attacker.com",
            "https://evil.pathbase-pr-1.fly.dev",
            "https://pathbase-pr-.fly.dev",
            "https://pathbase-pr.fly.dev",
            "https://xpathbase-pr-1.fly.dev",
            // Another scheme or port.
            "http://pathbase-pr-1.fly.dev",
            "https://pathbase-pr-1.fly.dev:8443",
            // Not written as a browser writes an origin.
            "https://PATHBASE-pr-1.fly.dev",
            "https://pathbase-pr-1.fly.dev/",
            "https://pathbase-pr-1.fly.dev:443",
            "https://pathbase-pr-1.fly.dev/path",
            "pathbase-pr-1.fly.dev",
            "null",
            "",
            // The pattern itself is no origin.
            "https://pathbase-pr-*.fly.dev",
        ] {
            assert!(!allows(not), "{not}");
        }
    }

    #[test]
    fn a_partial_star_keeps_its_port_and_its_text_around_the_star() {
        let listed = vec![
            normalize_queue_origin("http://dev-*.example.com:8080").unwrap(),
            normalize_queue_origin("https://*-web.a-*.example.com").unwrap(),
        ];
        let allows = |o: &str| queue_allows_origin(&listed, o);
        assert!(allows("http://dev-7.example.com:8080"));
        assert!(!allows("http://dev-7.example.com"));
        assert!(!allows("https://dev-7.example.com:8080"));
        assert!(allows("https://pr-9-web.a-x.example.com"));
        assert!(!allows("https://pr-9-web.a-.example.com"));
        assert!(!allows("https://-web.a-x.example.com"));
        assert!(!allows("https://pr-9-web.b-x.example.com"));
    }

    #[test]
    fn a_bare_star_matches_one_or_more_whole_labels_where_it_stands() {
        let allows = |pattern: &str, o: &str| {
            queue_allows_origin(&[normalize_queue_origin(pattern).unwrap()], o)
        };
        let first = "https://*.example.com";
        assert!(allows(first, "https://a.example.com"));
        assert!(allows(first, "https://a.b.example.com"));
        assert!(allows(first, "https://xn--bcher-kva.example.com"));
        for not in [
            "https://example.com",
            "https://aexample.com",
            "https://a.example.com.evil.dev",
            "https://a.example.org",
            "http://a.example.com",
            "https://a.example.com:8443",
            "https://A.example.com",
            "https://a_b.example.com",
            "https://*.example.com",
        ] {
            assert!(!allows(first, not), "{first} {not}");
        }

        let middle = "https://a.*.example.com";
        assert!(allows(middle, "https://a.x.example.com"));
        assert!(allows(middle, "https://a.x.y.example.com"));
        for not in [
            "https://a.example.com",
            "https://b.x.example.com",
            "https://x.a.x.example.com",
            "https://a.x.example.org",
        ] {
            assert!(!allows(middle, not), "{middle} {not}");
        }

        let ported = "http://*.example.com:8080";
        assert!(allows(ported, "http://a.b.example.com:8080"));
        assert!(!allows(ported, "http://a.example.com"));
        assert!(!allows(ported, "https://a.example.com:8080"));

        // A later literal that first fits too early: the `*` grows past it.
        let regrow = "https://*.a.b.example.com";
        assert!(allows(regrow, "https://a.a.b.a.b.example.com"));
        assert!(!allows(regrow, "https://a.b.example.com"));
    }

    #[test]
    fn several_bare_stars_each_take_at_least_one_label() {
        let allows = |pattern: &str, o: &str| {
            queue_allows_origin(&[normalize_queue_origin(pattern).unwrap()], o)
        };
        let two = "https://*.*.example.com";
        assert!(allows(two, "https://a.b.example.com"));
        assert!(allows(two, "https://a.b.c.d.example.com"));
        assert!(!allows(two, "https://a.example.com"));

        let apart = "https://*.a.*.example.com";
        assert!(allows(apart, "https://x.a.y.example.com"));
        assert!(allows(apart, "https://x.y.a.z.w.example.com"));
        assert!(allows(apart, "https://a.a.a.example.com"));
        assert!(!allows(apart, "https://a.y.example.com"));
        assert!(!allows(apart, "https://x.a.example.com"));
        assert!(!allows(apart, "https://x.b.y.example.com"));
    }

    #[test]
    fn bare_and_partial_stars_mix() {
        let allows = |pattern: &str, o: &str| {
            queue_allows_origin(&[normalize_queue_origin(pattern).unwrap()], o)
        };
        let mixed = "https://web.*.pr-*.example.com";
        assert!(allows(mixed, "https://web.eu.pr-7.example.com"));
        assert!(allows(mixed, "https://web.eu.west.pr-7.example.com"));
        for not in [
            "https://web.pr-7.example.com",
            "https://web.eu.pr-.example.com",
            "https://web.eu.pr-7.x.example.com",
            "https://api.eu.pr-7.example.com",
        ] {
            assert!(!allows(mixed, not), "{mixed} {not}");
        }
        let suffix = "https://*.api-*.example.com";
        assert!(allows(suffix, "https://a.b.api-eu.example.com"));
        assert!(!allows(suffix, "https://api-eu.example.com"));
        let fly = "https://*.fly.dev";
        assert!(allows(fly, "https://pathbase-pr-538.fly.dev"));
        assert!(allows(fly, "https://pathbase-pr-1.evil.fly.dev"));
        assert!(!allows(fly, "https://fly.dev"));
    }

    #[test]
    fn a_pattern_matches_dns_names_never_an_ip_address() {
        // Two labels follow the `*`, so the pattern is kept, but an
        // origin whose host is an IP address is not a name it matches.
        let listed = vec![normalize_queue_origin("https://10.*.0.1").unwrap()];
        assert!(!queue_allows_origin(&listed, "https://10.1.0.1"));
        assert!(!queue_allows_origin(&listed, "https://10.1.2.0.1"));
    }

    #[test]
    fn matching_stays_bounded_on_a_pathological_host() {
        // 60 `*` labels against 120 labels before `example.com`: a matcher
        // that tried every split of the labels among the `*`s would not
        // finish; this one makes at most a pattern's labels of comparisons
        // per label of the host.
        let pattern = format!("https://{}x.example.com", "*.".repeat(60));
        let listed = vec![normalize_queue_origin(&pattern).unwrap()];
        let miss = format!("https://{}example.com", "b.".repeat(120));
        assert!(!queue_allows_origin(&listed, &miss));
        let hit = format!("https://{}x.example.com", "b.".repeat(118));
        assert!(queue_allows_origin(&listed, &hit));
        let short = format!("https://{}x.example.com", "b.".repeat(59));
        assert!(!queue_allows_origin(&listed, &short));
        // A host longer than a DNS name is matched by nothing.
        let any = vec![normalize_queue_origin("https://*.example.com").unwrap()];
        let long = format!("https://{}example.com", "b.".repeat(122));
        assert!(!queue_allows_origin(&any, &long));
        let longest = format!("https://{}example.com", "b.".repeat(121));
        assert!(queue_allows_origin(&any, &longest));
    }

    #[test]
    fn exact_origins_match_only_themselves() {
        let listed = vec![
            normalize_queue_origin("https://app.example.com").unwrap(),
            normalize_queue_origin("http://localhost:5173").unwrap(),
        ];
        assert!(queue_allows_origin(&listed, "https://app.example.com"));
        assert!(queue_allows_origin(&listed, "http://localhost:5173"));
        assert!(!queue_allows_origin(
            &listed,
            "https://app.example.com:8443"
        ));
        assert!(!queue_allows_origin(&listed, "http://app.example.com"));
        assert!(!queue_allows_origin(&listed, "https://x.app.example.com"));
        assert!(!queue_allows_origin(&listed, "http://localhost:5174"));
        assert!(!queue_allows_origin(&[], "https://app.example.com"));
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

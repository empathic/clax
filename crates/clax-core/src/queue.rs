//! Report queues and external artifacts (spec 2026-10-27-clax-sdk-design
//! §6, §7): the records a queue's routes serve, their limits, and the
//! checks of what a deployer's app sends. The store keeps them
//! (`clax_store::queues`, `clax_store::external`).

use crate::anchor::Anchor;
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

/// Where a report stands (§7.3): `resolved` once its thread is; else
/// `claimed` while a live lease holds it; else `open`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Open,
    Claimed,
    Resolved,
}

impl State {
    /// The served name.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Claimed => "claimed",
            State::Resolved => "resolved",
        }
    }

    /// The state named `s`.
    pub fn parse(s: &str) -> Option<State> {
        match s {
            "open" => Some(State::Open),
            "claimed" => Some(State::Claimed),
            "resolved" => Some(State::Resolved),
            _ => None,
        }
    }
}

/// A queue as a member reads it, with the reader's own role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
/// [`AUTHOR_REPORTER`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportComment {
    pub id: String,
    pub author_kind: String,
    pub author_name: String,
    pub body: String,
    pub created_at: String,
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
}

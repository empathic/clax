//! What a report carries beyond its anchor and comments (spec
//! 2026-10-29-feedback-loop-design §5, §6): the context the reporter's
//! browser captured, the diagnostics beside it, the server's stamp, the
//! reporter's profile, and the fix attempts and their verdicts. The store
//! keeps them (`clax_store::queues`); the queue routes check what is sent
//! with the functions here.

use crate::scrub::{scrub, scrub_url};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;

/// The largest report context, as compact JSON, in bytes (§5.1).
pub const MAX_CONTEXT_BYTES: usize = 16 * 1024;
/// The most lines a report's diagnostics hold (§5.2).
pub const MAX_DIAG_LINES: usize = 200;
/// The largest diagnostics part, in bytes (§5.2).
pub const MAX_DIAG_BYTES: usize = 256 * 1024;
/// The longest field of a reporter profile, in bytes (§5.4).
pub const MAX_PROFILE_FIELD_BYTES: usize = 256;
/// The longest fix summary, in bytes (§6.2).
pub const MAX_FIX_SUMMARY_BYTES: usize = 4 * 1024;
/// The longest link of a fix (`url`, `verify_url`), in bytes.
pub const MAX_FIX_URL_BYTES: usize = 2048;
/// The longest other field of a fix (`commit`, `release`, `verify_after`),
/// in bytes.
pub const MAX_FIX_FIELD_BYTES: usize = 256;
/// The longest `captured_at`, and the longest diagnostics `at`, in bytes.
pub const MAX_STAMP_BYTES: usize = 64;

/// What the reporter's browser said at capture (§5.1): filled by the SDK
/// and the host app, checked by [`parse_context`], stored with the report
/// and never changed. Its top level admits no other key; the objects inside
/// it ignore keys they do not know.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportContext {
    /// The shape's version: 1.
    pub v: u8,
    /// When the capture was made, RFC 3339.
    pub captured_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<Release>,
    /// The host app's feature flags at capture.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, FlagValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Environment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<CaptureInfo>,
    /// Opaque JSON the host app adds; every string in it is scrubbed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<serde_json::Value>,
}

/// The release of the host app the reporter ran, as the host names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

/// A feature flag's value: a boolean, a number or a string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FlagValue {
    Bool(bool),
    Num(f64),
    Str(String),
}

/// The page and browser the capture was made in. `url` keeps its query
/// keys, never their values ([`scrub_url`]); `route` and `title` are
/// [`scrub`]bed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll: Option<Scroll>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// `light` or `dark`, as the page's `prefers-color-scheme` matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<String>,
}

/// The viewport in CSS pixels, and its device pixel ratio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub w: f64,
    pub h: f64,
    pub dpr: f64,
}

/// The page's scroll offsets in CSS pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scroll {
    pub x: f64,
    pub y: f64,
}

/// How the report's clip was captured (§4.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureInfo {
    pub mode: CaptureMode,
    pub method: CaptureMethod,
    /// What the render left out, and where (§4.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<Omitted>,
}

/// What the reporter captured (§4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    Element,
    Range,
    Area,
    Screen,
}

/// How the clip was made (§4.1): `dom`, a render of the page's DOM, or
/// `display`, the browser's screen capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMethod {
    Dom,
    Display,
}

/// A part of the page a DOM render left out (§4.2): its kind (`iframe`,
/// `canvas`, `video`, a masked element, …) and its box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Omitted {
    pub kind: String,
    pub rect: Rect,
}

/// A box in CSS pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// What the server that received a report says about itself (§5.1), as
/// its `/healthz` reports it, and when it received the report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStamp {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    pub received_at: String,
}

fn bad_context(message: impl Into<String>) -> CoreError {
    CoreError::InvalidContext(message.into())
}

fn too_large_context() -> CoreError {
    bad_context(format!(
        "a report context is at most {MAX_CONTEXT_BYTES} bytes as compact JSON"
    ))
}

/// `s` [`scrub`]bed in place.
fn scrub_in_place(s: &mut String) {
    if let Cow::Owned(o) = scrub(s) {
        *s = o;
    }
}

/// The report context `json`, scrubbed: `environment.url` through
/// [`scrub_url`]; `environment.route`, `environment.title` and every
/// string in `app` through [`scrub`]. `release` and `flags` are the host's
/// own and kept as sent. The size limit applies to the context as sent,
/// before scrubbing.
///
/// # Errors
/// [`CoreError::InvalidContext`] when `json` is over [`MAX_CONTEXT_BYTES`]
/// as compact JSON, names a top-level key the shape lacks, has a field of
/// the wrong type, has a `v` other than 1, or a `captured_at` that is not
/// an RFC 3339 time of at most [`MAX_STAMP_BYTES`] bytes.
pub fn parse_context(json: &str) -> Result<ReportContext> {
    // However much of it is whitespace, a context this long is over.
    if json.len() > 4 * MAX_CONTEXT_BYTES {
        return Err(too_large_context());
    }
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| bad_context(e.to_string()))?;
    if serde_json::to_string(&value)
        .expect("JSON values serialise")
        .len()
        > MAX_CONTEXT_BYTES
    {
        return Err(too_large_context());
    }
    let mut ctx: ReportContext =
        serde_json::from_value(value).map_err(|e| bad_context(e.to_string()))?;
    if ctx.v != 1 {
        return Err(bad_context(format!(
            "report context version {} is unknown; version 1 is",
            ctx.v
        )));
    }
    if ctx.captured_at.len() > MAX_STAMP_BYTES
        || chrono::DateTime::parse_from_rfc3339(&ctx.captured_at).is_err()
    {
        return Err(bad_context("captured_at is an RFC 3339 time"));
    }
    if let Some(env) = ctx.environment.as_mut() {
        if let Some(u) = env.url.as_mut() {
            *u = scrub_url(u);
        }
        for s in [env.route.as_mut(), env.title.as_mut()]
            .into_iter()
            .flatten()
        {
            scrub_in_place(s);
        }
    }
    if let Some(app) = ctx.app.as_mut() {
        scrub_strings(app);
    }
    Ok(ctx)
}

/// Every string in `v`, at any depth, [`scrub`]bed; object keys are kept.
fn scrub_strings(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => scrub_in_place(s),
        serde_json::Value::Array(a) => a.iter_mut().for_each(scrub_strings),
        serde_json::Value::Object(m) => m.values_mut().for_each(scrub_strings),
        _ => {}
    }
}

/// A line of a report's diagnostics (§5.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagLine {
    /// When it happened, as the browser stamped it.
    pub at: String,
    /// One of [`DIAG_LEVELS`].
    pub level: String,
    /// One of [`DIAG_KINDS`]: `console`, `error` (an uncaught error),
    /// `rejection` (an unhandled promise rejection) or `request` (a failed
    /// request).
    pub kind: String,
    /// What was logged, scrubbed.
    pub text: String,
}

/// The levels a diagnostics line may name.
pub const DIAG_LEVELS: &[&str] = &["debug", "info", "warn", "error"];
/// The kinds a diagnostics line may name.
pub const DIAG_KINDS: &[&str] = &["console", "error", "rejection", "request"];

fn bad_diag(message: impl Into<String>) -> CoreError {
    CoreError::InvalidDiagnostics(message.into())
}

/// The diagnostics part `bytes`: UTF-8 JSON lines, one [`DiagLine`] each
/// (blank lines, and a `\r` before a `\n`, are ignored), with each `text`
/// [`scrub`]bed.
///
/// # Errors
/// [`CoreError::InvalidDiagnostics`] when `bytes` is over
/// [`MAX_DIAG_BYTES`] or not UTF-8, holds more than [`MAX_DIAG_LINES`]
/// lines, or holds a line that is not a [`DiagLine`] with a known level
/// and kind and an `at` of at most [`MAX_STAMP_BYTES`] bytes.
pub fn parse_diagnostics(bytes: &[u8]) -> Result<Vec<DiagLine>> {
    if bytes.len() > MAX_DIAG_BYTES {
        return Err(bad_diag(format!(
            "diagnostics are at most {MAX_DIAG_BYTES} bytes"
        )));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| bad_diag("diagnostics are UTF-8"))?;
    let mut out = Vec::new();
    for (n, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim().is_empty() {
            continue;
        }
        if out.len() == MAX_DIAG_LINES {
            return Err(bad_diag(format!(
                "diagnostics are at most {MAX_DIAG_LINES} lines"
            )));
        }
        let at = |m: String| bad_diag(format!("line {}: {m}", n + 1));
        let mut d: DiagLine = serde_json::from_str(line).map_err(|e| at(e.to_string()))?;
        if !DIAG_LEVELS.contains(&d.level.as_str()) {
            return Err(at(format!("level is one of {}", DIAG_LEVELS.join(", "))));
        }
        if !DIAG_KINDS.contains(&d.kind.as_str()) {
            return Err(at(format!("kind is one of {}", DIAG_KINDS.join(", "))));
        }
        if d.at.len() > MAX_STAMP_BYTES {
            return Err(at(format!("at is at most {MAX_STAMP_BYTES} bytes")));
        }
        scrub_in_place(&mut d.text);
        out.push(d);
    }
    Ok(out)
}

/// Who a reporter is, as the integrator's backend asserts it when minting
/// the reporter token (§5.4). Shown to triage members, never to
/// reporters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReporterProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// A GitHub login, for asking the reporter to review a fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    /// An `https:` or `http:` image URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

impl ReporterProfile {
    /// Whether the profile holds no field.
    pub fn is_empty(&self) -> bool {
        self == &ReporterProfile::default()
    }

    /// `Ok` when every field is at most [`MAX_PROFILE_FIELD_BYTES`] bytes
    /// and `avatar_url` is an `https:` or `http:` URL.
    ///
    /// # Errors
    /// [`CoreError::InvalidProfile`] otherwise.
    pub fn check(&self) -> Result<()> {
        for (name, v) in [
            ("name", &self.name),
            ("email", &self.email),
            ("github", &self.github),
            ("avatar_url", &self.avatar_url),
        ] {
            if v.as_ref()
                .is_some_and(|s| s.len() > MAX_PROFILE_FIELD_BYTES)
            {
                return Err(CoreError::InvalidProfile(format!(
                    "a profile's {name} is at most {MAX_PROFILE_FIELD_BYTES} bytes"
                )));
            }
        }
        if let Some(u) = &self.avatar_url
            && !is_http_url(u)
        {
            return Err(CoreError::InvalidProfile(
                "a profile's avatar_url is an https: or http: URL".into(),
            ));
        }
        Ok(())
    }
}

/// Whether `s` is an absolute `https:` or `http:` URL with a host.
fn is_http_url(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| matches!(u.scheme(), "https" | "http") && u.has_host())
}

/// A proposed fix (§6.2): what was done, and where to see and check it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewFix {
    /// Plain text, posted to the report as a comment the reporter sees.
    pub summary: String,
    /// The change: a pull request, a commit page, a ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The release the fix ships in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// Where the reporter checks the fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_url: Option<String>,
    /// When the reporter can check it, such as `release:0.6.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_after: Option<String>,
}

impl NewFix {
    /// `Ok` for a fix Clax records: a summary that is not blank and is at
    /// most [`MAX_FIX_SUMMARY_BYTES`] bytes; `url` and `verify_url`, when
    /// given, `https:` or `http:` URLs of at most [`MAX_FIX_URL_BYTES`]
    /// bytes; `commit`, `release` and `verify_after` at most
    /// [`MAX_FIX_FIELD_BYTES`] bytes.
    ///
    /// # Errors
    /// `invalid_fix` otherwise.
    pub fn check(&self) -> Result<()> {
        let bad = |m: String| CoreError::invalid("invalid_fix", m);
        if self.summary.trim().is_empty() {
            return Err(bad("a fix needs a summary".into()));
        }
        if self.summary.len() > MAX_FIX_SUMMARY_BYTES {
            return Err(bad(format!(
                "a fix summary is at most {MAX_FIX_SUMMARY_BYTES} bytes"
            )));
        }
        for (name, v) in [("url", &self.url), ("verify_url", &self.verify_url)] {
            if let Some(u) = v
                && (u.len() > MAX_FIX_URL_BYTES || !is_http_url(u))
            {
                return Err(bad(format!(
                    "a fix's {name} is an https: or http: URL of at most {MAX_FIX_URL_BYTES} bytes"
                )));
            }
        }
        for (name, v) in [
            ("commit", &self.commit),
            ("release", &self.release),
            ("verify_after", &self.verify_after),
        ] {
            if v.as_ref().is_some_and(|s| s.len() > MAX_FIX_FIELD_BYTES) {
                return Err(bad(format!(
                    "a fix's {name} is at most {MAX_FIX_FIELD_BYTES} bytes"
                )));
            }
        }
        Ok(())
    }
}

/// What became of a fix attempt (§6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixOutcome {
    /// Waiting for the reporter's verdict.
    Pending,
    /// The reporter said it is fixed.
    Verified,
    /// The reporter said it is still broken.
    Rejected,
}

impl FixOutcome {
    /// The stored and served name.
    pub fn as_str(self) -> &'static str {
        match self {
            FixOutcome::Pending => "pending",
            FixOutcome::Verified => "verified",
            FixOutcome::Rejected => "rejected",
        }
    }

    /// The outcome named `s`.
    pub fn parse(s: &str) -> Option<FixOutcome> {
        match s {
            "pending" => Some(FixOutcome::Pending),
            "verified" => Some(FixOutcome::Verified),
            "rejected" => Some(FixOutcome::Rejected),
            _ => None,
        }
    }
}

/// A fix proposed on a report (§6.2), numbered from 1 in the order
/// proposed, with its verdict once given (§6.3): the verdict's note, the
/// context of whoever gave it, and when.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixAttempt {
    pub n: u32,
    #[serde(flatten)]
    pub fix: NewFix,
    pub proposer_name: String,
    pub created_at: String,
    pub outcome: FixOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_context: Option<ReportContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_at: Option<String>,
}

/// Why a report was closed (§6.1): `fixed` when a fix was verified or
/// triage closed it as fixed; the others when triage closed it without a
/// verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolveReason {
    Fixed,
    Duplicate,
    Wontfix,
    CannotReproduce,
    Other,
}

impl ResolveReason {
    /// Every reason, in the spec's order.
    pub const ALL: &'static [ResolveReason] = &[
        ResolveReason::Fixed,
        ResolveReason::Duplicate,
        ResolveReason::Wontfix,
        ResolveReason::CannotReproduce,
        ResolveReason::Other,
    ];

    /// The stored and served name.
    pub fn as_str(self) -> &'static str {
        match self {
            ResolveReason::Fixed => "fixed",
            ResolveReason::Duplicate => "duplicate",
            ResolveReason::Wontfix => "wontfix",
            ResolveReason::CannotReproduce => "cannot_reproduce",
            ResolveReason::Other => "other",
        }
    }

    /// The reason named `s`.
    pub fn parse(s: &str) -> Option<ResolveReason> {
        ResolveReason::ALL.iter().copied().find(|r| r.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrub::REDACTED;

    fn ctx(extra: &str) -> String {
        format!(r#"{{"v":1,"captured_at":"2026-10-29T07:12:03.120Z"{extra}}}"#)
    }

    #[test]
    fn context_with_unknown_key_is_invalid_context() {
        let e = parse_context(&ctx(r#","nope":1"#)).unwrap_err();
        assert!(matches!(e, CoreError::InvalidContext(_)), "{e:?}");
    }

    #[test]
    fn context_over_16k_is_invalid_context() {
        let big = "x".repeat(MAX_CONTEXT_BYTES);
        let e = parse_context(&ctx(&format!(r#","app":{{"s":"{big}"}}"#))).unwrap_err();
        assert!(matches!(e, CoreError::InvalidContext(_)), "{e:?}");
        let fits = "x".repeat(MAX_CONTEXT_BYTES - 200);
        assert!(parse_context(&ctx(&format!(r#","app":{{"s":"{fits}"}}"#))).is_ok());
    }

    #[test]
    fn context_v2_is_invalid_context() {
        let e = parse_context(r#"{"v":2,"captured_at":"2026-10-29T07:12:03.120Z"}"#).unwrap_err();
        assert!(matches!(e, CoreError::InvalidContext(_)), "{e:?}");
        let e = parse_context(r#"{"v":1,"captured_at":"yesterday"}"#).unwrap_err();
        assert!(matches!(e, CoreError::InvalidContext(_)), "{e:?}");
    }

    #[test]
    fn context_flags_take_bool_string_number() {
        let c = parse_context(&ctx(
            r#","flags":{"claxbot":true,"new_editor":"variant-b","ratio":0.5}"#,
        ))
        .unwrap();
        assert_eq!(c.flags["claxbot"], FlagValue::Bool(true));
        assert_eq!(c.flags["new_editor"], FlagValue::Str("variant-b".into()));
        assert_eq!(c.flags["ratio"], FlagValue::Num(0.5));
        for bad in [r#"{"a":null}"#, r#"{"a":[1]}"#, r#"{"a":{"b":1}}"#] {
            let e = parse_context(&ctx(&format!(r#","flags":{bad}"#))).unwrap_err();
            assert!(matches!(e, CoreError::InvalidContext(_)), "{bad}: {e:?}");
        }
    }

    #[test]
    fn context_scrubs_url_query_values() {
        let c = parse_context(&ctx(
            r#","release":{"commit":"da39a3ee5e6b4b0d3255bfef95601890afd80709","channel":"prod"},
               "environment":{"url":"https://clax.dev/a/x?token=s&tab=2#f","title":"mail ahk@example.com",
                 "viewport":{"w":1440,"h":900,"dpr":2},"scroll":{"x":0,"y":340.5}},
               "capture":{"mode":"screen","method":"dom","omitted":[{"kind":"iframe","rect":{"x":1,"y":2,"w":3,"h":4}}]},
               "app":{"note":"password=hunter2","list":["Bearer abcdefghijkl"],"n":3}"#,
        ))
        .unwrap();
        let env = c.environment.as_ref().unwrap();
        assert_eq!(
            env.url.as_deref(),
            Some("https://clax.dev/a/x?token=‹redacted›&tab=‹redacted›#f")
        );
        assert_eq!(env.title.as_deref(), Some("mail ‹redacted›"));
        assert_eq!(env.viewport.as_ref().unwrap().dpr, 2.0);
        // A release names its commit in the clear.
        assert_eq!(
            c.release.as_ref().unwrap().commit.as_deref(),
            Some("da39a3ee5e6b4b0d3255bfef95601890afd80709")
        );
        let capture = c.capture.as_ref().unwrap();
        assert_eq!(capture.mode, CaptureMode::Screen);
        assert_eq!(capture.method, CaptureMethod::Dom);
        assert_eq!(capture.omitted[0].rect.w, 3.0);
        let app = c.app.as_ref().unwrap();
        assert_eq!(app["note"], format!("password={REDACTED}"));
        assert_eq!(app["list"][0], format!("Bearer {REDACTED}"));
        assert_eq!(app["n"], 3);
        // What is stored reads back as the same context.
        let back: ReportContext =
            serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn diagnostics_over_200_lines_is_invalid() {
        let line =
            r#"{"at":"2026-10-29T07:12:03.120Z","level":"info","kind":"console","text":"hi"}"#;
        let ok = vec![line; MAX_DIAG_LINES].join("\n");
        assert_eq!(
            parse_diagnostics(ok.as_bytes()).unwrap().len(),
            MAX_DIAG_LINES
        );
        let over = vec![line; MAX_DIAG_LINES + 1].join("\n");
        let e = parse_diagnostics(over.as_bytes()).unwrap_err();
        assert!(matches!(e, CoreError::InvalidDiagnostics(_)), "{e:?}");
        let big = format!(
            r#"{{"at":"t","level":"info","kind":"console","text":"{}"}}"#,
            "x".repeat(MAX_DIAG_BYTES)
        );
        let e = parse_diagnostics(big.as_bytes()).unwrap_err();
        assert!(matches!(e, CoreError::InvalidDiagnostics(_)), "{e:?}");
        for bad in [
            r#"{"at":"t","level":"loud","kind":"console","text":"x"}"#,
            r#"{"at":"t","level":"info","kind":"telepathy","text":"x"}"#,
            r#"{"at":"t","level":"info","kind":"console","text":"x","extra":1}"#,
            "not json",
        ] {
            let e = parse_diagnostics(bad.as_bytes()).unwrap_err();
            assert!(
                matches!(e, CoreError::InvalidDiagnostics(_)),
                "{bad}: {e:?}"
            );
        }
        assert!(matches!(
            parse_diagnostics(&[0xff, 0xfe]),
            Err(CoreError::InvalidDiagnostics(_))
        ));
    }

    #[test]
    fn diagnostics_text_is_scrubbed() {
        let body = concat!(
            r#"{"at":"2026-10-29T07:12:03.120Z","level":"warn","kind":"request","text":"GET /x?token=s3cret"}"#,
            "\r\n\n",
            r#"{"at":"2026-10-29T07:12:04.000Z","level":"error","kind":"rejection","text":"fine"}"#,
            "\n",
        );
        let lines = parse_diagnostics(body.as_bytes()).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, format!("GET /x?token={REDACTED}"));
        assert_eq!(lines[0].level, "warn");
        assert_eq!(lines[1].text, "fine");
        assert!(parse_diagnostics(b"").unwrap().is_empty());
    }

    #[test]
    fn profile_field_over_256_bytes_is_invalid_profile() {
        let ok = ReporterProfile {
            name: Some("Alex".into()),
            email: Some("ahk@example.com".into()),
            github: Some("ahk".into()),
            avatar_url: Some("https://avatars.example/ahk.png".into()),
        };
        ok.check().unwrap();
        let long = "x".repeat(MAX_PROFILE_FIELD_BYTES + 1);
        for p in [
            ReporterProfile {
                name: Some(long.clone()),
                ..ok.clone()
            },
            ReporterProfile {
                github: Some(long.clone()),
                ..ok.clone()
            },
            ReporterProfile {
                avatar_url: Some("javascript:alert(1)".into()),
                ..ok.clone()
            },
        ] {
            let e = p.check().unwrap_err();
            assert!(matches!(e, CoreError::InvalidProfile(_)), "{e:?}");
        }
        assert_eq!(
            serde_json::to_value(ReporterProfile::default()).unwrap(),
            serde_json::json!({})
        );
    }

    fn fix(summary: &str) -> NewFix {
        NewFix {
            summary: summary.into(),
            url: None,
            commit: None,
            release: None,
            verify_url: None,
            verify_after: None,
        }
    }

    #[test]
    fn fix_without_summary_is_invalid() {
        fix("Fixed the overflow").check().unwrap();
        for s in ["", "   ", &"x".repeat(MAX_FIX_SUMMARY_BYTES + 1)] {
            let e = fix(s).check().unwrap_err();
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "invalid_fix",
                        ..
                    }
                ),
                "{e:?}"
            );
        }
    }

    #[test]
    fn fix_url_must_be_http() {
        let ok = NewFix {
            url: Some("https://github.com/empathic/clax/pull/123".into()),
            verify_url: Some("http://localhost:5173/a/x".into()),
            ..fix("s")
        };
        ok.check().unwrap();
        for bad in [
            "javascript:alert(1)",
            "ftp://x.example/",
            "not a url",
            "data:text/html,x",
        ] {
            for f in [
                NewFix {
                    url: Some(bad.into()),
                    ..fix("s")
                },
                NewFix {
                    verify_url: Some(bad.into()),
                    ..fix("s")
                },
            ] {
                let e = f.check().unwrap_err();
                assert!(
                    matches!(
                        e,
                        CoreError::Invalid {
                            code: "invalid_fix",
                            ..
                        }
                    ),
                    "{bad}: {e:?}"
                );
            }
        }
    }

    #[test]
    fn fix_attempts_and_reasons_serialize_flat() {
        let a = FixAttempt {
            n: 1,
            fix: NewFix {
                url: Some("https://github.com/x/y/pull/1".into()),
                ..fix("s")
            },
            proposer_name: "claude".into(),
            created_at: "2026-10-29T07:12:03.120Z".into(),
            outcome: FixOutcome::Pending,
            verdict_note: None,
            verdict_context: None,
            verdict_at: None,
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["summary"], "s");
        assert_eq!(v["url"], "https://github.com/x/y/pull/1");
        assert_eq!(v["outcome"], "pending");
        assert_eq!(serde_json::from_value::<FixAttempt>(v).unwrap(), a);
        for r in [
            ResolveReason::Fixed,
            ResolveReason::Duplicate,
            ResolveReason::Wontfix,
            ResolveReason::CannotReproduce,
            ResolveReason::Other,
        ] {
            assert_eq!(ResolveReason::parse(r.as_str()), Some(r));
            assert_eq!(serde_json::to_value(r).unwrap(), r.as_str());
        }
        assert_eq!(ResolveReason::CannotReproduce.as_str(), "cannot_reproduce");
        for o in [
            FixOutcome::Pending,
            FixOutcome::Verified,
            FixOutcome::Rejected,
        ] {
            assert_eq!(FixOutcome::parse(o.as_str()), Some(o));
            assert_eq!(serde_json::to_value(o).unwrap(), o.as_str());
        }
    }
}

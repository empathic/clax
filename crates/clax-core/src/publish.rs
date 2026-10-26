//! Publish request validation: path safety, encodings, size caps, content types.

use crate::{CoreError, Result};
use base64::Engine;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Largest decoded size of a single file.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Largest total decoded size of all files in one publish.
pub const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;
/// Longest version label, in characters.
pub const MAX_LABEL_CHARS: usize = 60;
/// Longest artifact title, in characters: a derived title's bound too.
pub const MAX_TITLE_CHARS: usize = MAX_DERIVED_TITLE_CHARS;
/// Longest artifact description, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 1000;
/// Longest artifact icon word, in characters.
pub const MAX_ICON_CHARS: usize = 40;
/// The entry file every version must supply.
pub const INDEX: &str = "index.html";

/// How a file's `content` string encodes its bytes.
#[derive(Clone, Debug, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    #[default]
    Utf8,
    Base64,
}

/// One file in a publish request.
#[derive(Clone, Debug, Deserialize)]
pub struct FileInput {
    pub content: String,
    #[serde(default)]
    pub encoding: Encoding,
    pub content_type: Option<String>,
}

/// An unvalidated publish body. A `None` file value removes that path from the
/// carried-forward set.
#[derive(Clone, Debug, Deserialize, Default)]
pub struct PublishRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub label: Option<String>,
    pub if_version: Option<u32>,
    pub capabilities: Option<serde_json::Value>,
    #[serde(default)]
    pub files: BTreeMap<String, Option<FileInput>>,
    /// A short change note for the version's changelog.
    #[serde(default)]
    pub note: Option<String>,
    /// IDs of the artifact's threads this version addresses.
    #[serde(default)]
    pub addresses: Option<Vec<String>>,
    /// The new artifact's Comments switch, on when absent: set by the
    /// request that creates an artifact (`POST /api/artifacts`) alone, and
    /// not carried into [`ValidatedPublish`].
    #[serde(default)]
    pub comments: Option<bool>,
    /// The artifact the new one comes from, recorded as its lineage at
    /// that artifact's current version: set by the request that creates
    /// an artifact on a hosted Clax (`POST /api/artifacts`) alone, and not
    /// carried into [`ValidatedPublish`].
    #[serde(default)]
    pub forked_from: Option<String>,
}

/// A file's decoded bytes and resolved content type.
#[derive(Clone, Debug)]
pub struct DecodedFile {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// A validated change to one path.
#[derive(Clone, Debug)]
pub enum FileChange {
    Put(DecodedFile),
    Remove,
}

/// A publish request that passed [`validate`].
#[derive(Clone, Debug)]
pub struct ValidatedPublish {
    pub title: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub label: Option<String>,
    pub if_version: Option<u32>,
    pub capabilities: Option<serde_json::Value>,
    pub files: BTreeMap<String, FileChange>,
    /// The note after [`crate::working::clean_line`].
    pub note: Option<String>,
    /// The note was cut to [`crate::changelog::MAX_NOTE_CHARS`].
    pub note_truncated: bool,
    /// Thread IDs named explicitly; each must be a thread of the artifact.
    pub addresses: Vec<String>,
    /// Threads the publishing session was marked working on (empty from
    /// [`validate`]; the caller fills it). Ones that no longer exist are skipped.
    pub working_threads: Vec<String>,
    /// The publisher's person (public ID); `None` for the owner, or to take
    /// the publishing session's person. The caller fills it.
    pub person: Option<String>,
    /// The page published this version of itself (the caller fills it).
    pub by_page: bool,
    /// Paths of the previous version the publisher's reader may not see
    /// (the caller fills it; spec 2026-10-08-multi-agent-multi-person
    /// §21.7). They never constrain the publish: a put that collides with
    /// one (the same name in another letter case, a file beneath it, or a
    /// file over its directory) drops it, as a remove would, where a
    /// collision with any other path is refused. So a publish never
    /// confirms a withheld name by refusing it.
    pub hidden: Vec<String>,
    /// The mark of the job step that publishes it (the caller fills it):
    /// the content store keeps it with the version's content
    /// ([`crate::content::VersionMeta::step`]).
    pub step: Option<String>,
    /// The idempotency key an agent token named for this publish (the
    /// caller fills it): recorded in the transaction that records the
    /// version, so the version and its key are written together.
    pub publish_key: Option<PublishKey>,
}

/// An agent token's idempotency key for a publish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishKey {
    /// The agent token's grant.
    pub grant_id: String,
    pub key: String,
}

/// What a restore (`versions:restore`) may set beside the restored files,
/// each field as a publish sets it; `None` keeps the artifact's title,
/// icon and capabilities, and the restore's own label and note.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RestoreDeclaration {
    pub title: Option<String>,
    pub icon: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub label: Option<String>,
    pub note: Option<String>,
}

impl RestoreDeclaration {
    /// Checks it as [`validate`] checks a publish's same fields
    /// ([`check_declared`], [`check_meta_text`]). Answers it with whether
    /// the note was cut.
    pub fn validated(self) -> Result<(Self, bool)> {
        let (note, truncated) = check_declared(
            self.capabilities.as_ref(),
            self.label.as_deref(),
            self.note.as_deref(),
        )?;
        check_meta_text(self.title.as_deref(), None, self.icon.as_deref())?;
        Ok((Self { note, ..self }, truncated))
    }
}

/// The checks a publish ([`validate`]) and a restore
/// ([`RestoreDeclaration::validated`]) both run on what they declare beside
/// files: the capabilities validate, the label is at most
/// [`MAX_LABEL_CHARS`], and the note is cleaned and cut to the changelog's
/// length. Answers the cleaned note and whether it was cut.
fn check_declared(
    capabilities: Option<&serde_json::Value>,
    label: Option<&str>,
    note: Option<&str>,
) -> Result<(Option<String>, bool)> {
    if let Some(caps) = capabilities {
        crate::capabilities::validate(caps)?;
    }
    if let Some(label) = label
        && label.chars().count() > MAX_LABEL_CHARS
    {
        return Err(CoreError::invalid(
            "label_too_long",
            format!("label exceeds {MAX_LABEL_CHARS} characters"),
        ));
    }
    Ok(match note {
        Some(n) => crate::working::clean_line(n, crate::changelog::MAX_NOTE_CHARS),
        None => (None, false),
    })
}

/// Accepts a relative path of one or more non-empty segments, with no `.` or
/// `..` segments, no control characters or U+2028/U+2029 line and paragraph
/// separators, and forward slashes only.
pub fn check_path(path: &str) -> Result<()> {
    let bad = || {
        CoreError::invalid(
            "invalid_path",
            format!("'{path}' is not a safe relative path"),
        )
    };
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') || path.contains('\\') {
        return Err(bad());
    }
    for seg in path.split('/') {
        if seg.is_empty()
            || seg == "."
            || seg == ".."
            || seg
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        {
            return Err(bad());
        }
    }
    Ok(())
}

/// The first path segment Clax keeps for itself on every site host: its
/// versions, drafts, assets, bridge and health live under `/_clax/`.
pub const RESERVED_SEGMENT: &str = "_clax";

/// The first path segment a legacy artifact's site host also keeps: its
/// assets' `/_blob/<id>` addresses, which pages made before site hosts use.
pub const LEGACY_RESERVED_SEGMENT: &str = "_blob";

/// [`check_path`], and a path that a site host serves itself is refused
/// (`invalid_path`, "'_clax/…' is reserved"): one whose first segment is
/// `_clax`, and for a `legacy` artifact (`artifacts.legacy_paths`) `_blob`.
pub fn check_path_for(path: &str, legacy: bool) -> Result<()> {
    check_path(path)?;
    let first = path.split('/').next().unwrap_or("");
    if first == RESERVED_SEGMENT || (legacy && first == LEGACY_RESERVED_SEGMENT) {
        return Err(CoreError::invalid(
            "invalid_path",
            format!("'{path}': '{first}/…' is reserved for Clax on the artifact's site"),
        ));
    }
    Ok(())
}

/// Largest accepted asset in bytes. The cap is inclusive: an asset of exactly this size is accepted.
pub const MAX_ASSET_BYTES: u64 = 20 * 1024 * 1024;
/// The most assets a fork copies: a version whose files name more is
/// refused `too_many_assets` before any is copied.
pub const MAX_FORK_ASSETS: usize = 256;
/// The most bytes of assets a fork copies, all its assets together: past
/// it the fork is refused `too_many_assets` before any is copied.
pub const MAX_FORK_ASSET_BYTES: u64 = 256 * 1024 * 1024;

/// Whether `content_type` (parameters after `;` ignored) is an accepted asset type.
pub fn is_asset_type(content_type: &str) -> bool {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    ct.starts_with("image/")
        || ct.starts_with("video/")
        || ct.starts_with("font/")
        || matches!(
            ct,
            "application/pdf"
                | "text/css"
                | "text/javascript"
                | "text/csv"
                | "text/markdown"
                | "application/json"
                | "text/plain"
        )
}

/// The content types a page's `assets` capability uploads
/// (web/contract/0.3.0/assets.d.ts), exact spellings, no parameters.
pub const PAGE_ASSET_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "video/mp4",
    "video/webm",
    "application/pdf",
    "font/woff2",
    "font/woff",
    "font/ttf",
    "font/otf",
    "image/svg+xml",
    "text/csv",
    "text/markdown",
    "application/json",
    "text/plain",
    "text/css",
    "text/javascript",
];

/// Largest SVG a page uploads, in bytes.
pub const PAGE_SVG_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Largest stylesheet or script a page uploads, in bytes.
pub const PAGE_CODE_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// How much of a binary or SVG upload is read to judge its start.
const SNIFF_BYTES: usize = 4096;

/// Whether the first non-blank byte, after a UTF-8 byte-order mark, is `<`.
fn starts_with_markup(b: &[u8]) -> bool {
    let b = b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b);
    b.iter()
        .find(|c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c'))
        .is_some_and(|c| *c == b'<')
}

/// Whether `head` opens an SVG document: `<svg` after any XML prolog,
/// comments and doctype.
fn opens_svg(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let mut s = text.trim_start_matches('\u{feff}');
    loop {
        s = s.trim_start();
        let skip = |open: &str, close: &str, s: &str| -> Option<usize> {
            let rest = s.get(open.len()..)?;
            if !s[..open.len()].eq_ignore_ascii_case(open) {
                return None;
            }
            rest.find(close).map(|i| open.len() + i + close.len())
        };
        // A doctype's internal subset (`[…]`) may hold `>` of its own.
        let doctype = |s: &str| -> Option<usize> {
            let end = skip("<!DOCTYPE", ">", s)?;
            match s[..end].find('[') {
                Some(open) => s[open..]
                    .find(']')
                    .and_then(|c| s[open + c..].find('>').map(|g| open + c + g + 1)),
                None => Some(end),
            }
        };
        let next = skip("<?", "?>", s)
            .or_else(|| skip("<!--", "-->", s))
            .or_else(|| doctype(s));
        match next {
            Some(n) => s = &s[n..],
            None => {
                return s.starts_with("<svg")
                    && s[4..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_whitespace() || c == '/' || c == '>');
            }
        }
    }
}

/// A page's upload (the `assets` capability, `x-clax-via: page`) checked
/// against the contract, which the page's own checks only mirror: the type
/// is one of [`PAGE_ASSET_TYPES`] (else `unsupported_type`); an SVG is at
/// most [`PAGE_SVG_MAX_BYTES`] and a stylesheet or script
/// [`PAGE_CODE_MAX_BYTES`] (else `asset_too_large`); a binary type does not
/// start with markup, and an SVG opens an `<svg>` element (else
/// `unsupported_type`); a text type is UTF-8 (else `invalid_request`), and a
/// stylesheet or script holds no NUL and does not start with markup (else
/// `unsupported_type`).
pub fn check_page_asset(content_type: &str, bytes: &[u8]) -> Result<()> {
    let ct = content_type;
    if !PAGE_ASSET_TYPES.contains(&ct) {
        return Err(CoreError::invalid(
            "unsupported_type",
            format!(
                "'{ct}' is not an accepted type; exact media types only: {}",
                PAGE_ASSET_TYPES.join(", ")
            ),
        ));
    }
    let code = matches!(ct, "text/css" | "text/javascript");
    let limit = match ct {
        "image/svg+xml" => PAGE_SVG_MAX_BYTES,
        _ if code => PAGE_CODE_MAX_BYTES,
        _ => MAX_ASSET_BYTES,
    };
    if bytes.len() as u64 > limit {
        return Err(CoreError::invalid(
            "asset_too_large",
            format!("a {ct} asset is at most {limit} bytes"),
        ));
    }
    let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
    let unsupported = |why: String| Err(CoreError::invalid("unsupported_type", why));
    if ct == "image/svg+xml" {
        return if opens_svg(head) {
            Ok(())
        } else {
            unsupported("the body is not an SVG document".into())
        };
    }
    let text = ct.starts_with("text/") || ct == "application/json";
    if !text {
        return if starts_with_markup(head) {
            unsupported(format!(
                "a {ct} body starts with markup; markup uploads only as image/svg+xml"
            ))
        } else {
            Ok(())
        };
    }
    if std::str::from_utf8(bytes).is_err() {
        return Err(CoreError::invalid(
            "invalid_request",
            format!("a {ct} file must be UTF-8; re-encode it before uploading"),
        ));
    }
    let nul = bytes.contains(&0);
    if code && (nul || starts_with_markup(bytes)) {
        return unsupported(format!(
            "a {ct} file must read as text and not start with markup"
        ));
    }
    if nul {
        return Err(CoreError::invalid(
            "invalid_request",
            format!(
                "a {ct} file must be UTF-8 (this one reads as UTF-16 or binary); re-encode it before uploading"
            ),
        ));
    }
    Ok(())
}

/// Content type inferred from the path's extension (as [`std::path::Path::extension`]
/// defines it, compared case-insensitively); JavaScript is always
/// `text/javascript`, and paths with no or an unknown extension are
/// `application/octet-stream`.
pub fn content_type_for(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("js") | Some("mjs") => "text/javascript".to_string(),
        Some(ext) => mime_guess::from_ext(ext)
            .first_raw()
            .unwrap_or("application/octet-stream")
            .to_string(),
        None => "application/octet-stream".to_string(),
    }
}

/// Longest title [`html_title`] returns, in characters.
pub const MAX_DERIVED_TITLE_CHARS: usize = 200;

/// The text of the first `<title>` element of `html`, for use as an artifact
/// title. `<!-- comments -->` and the contents of `<script>`, `<style>` and
/// `<svg>` elements are skipped (an unclosed one hides the rest of the page).
/// Tag names match in any case and may carry attributes; the five
/// entities `&amp;` `&lt;` `&gt;` `&quot;` `&apos;` are decoded once (any
/// other entity is kept as written); control characters other than
/// whitespace and hidden format characters ([`crate::text::is_hidden_char`])
/// are dropped, so the title passes [`check_meta_text`]; runs of whitespace
/// become one space; the result is trimmed and cut to
/// [`MAX_DERIVED_TITLE_CHARS`] characters. `None` when there is no closed
/// `<title>` element or its text is empty.
pub fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    let body_start = loop {
        let at = from + lower[from..].find('<')?;
        let rest = &lower[at..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            from = at + 4 + comment.find("-->")? + 3;
            continue;
        }
        let Some(name) = ["title", "script", "style", "svg"]
            .into_iter()
            .find(|n| tag_starts(rest, n))
        else {
            from = at + 1;
            continue;
        };
        let after = at + 1 + name.len();
        let gt = after + lower[after..].find('>')?;
        if name == "title" {
            break gt + 1;
        }
        if lower.as_bytes()[gt - 1] == b'/' {
            // Self-closing, as `<svg/>`: no contents to skip.
            from = gt + 1;
            continue;
        }
        let close = format!("</{name}");
        from = gt + 1 + lower[gt + 1..].find(&close)? + close.len();
    };
    let body_end = body_start + lower[body_start..].find("</title")?;
    let text: String = decode_basic_entities(&html[body_start..body_end])
        .chars()
        .filter(|&c| c.is_whitespace() || !(c.is_control() || crate::text::is_hidden_char(c)))
        .collect();
    let title: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_DERIVED_TITLE_CHARS)
        .collect();
    let title = title.trim_end().to_string();
    (!title.is_empty()).then_some(title)
}

/// True when `rest` (lowercased, starting at `<`) opens a `name` tag: the name
/// is followed by `>`, ASCII whitespace or `/`.
fn tag_starts(rest: &str, name: &str) -> bool {
    rest[1..].starts_with(name)
        && rest
            .as_bytes()
            .get(1 + name.len())
            .is_some_and(|b| *b == b'>' || *b == b'/' || b.is_ascii_whitespace())
}

/// `s` with `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&apos;` decoded in one pass.
fn decode_basic_entities(s: &str) -> String {
    const ENTITIES: [(&str, char); 5] = [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&apos;", '\''),
    ];
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match ENTITIES.iter().find(|(e, _)| rest.starts_with(e)) {
            Some((e, c)) => {
                out.push(*c);
                rest = &rest[e.len()..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The title of a new artifact: `Some` non-blank text.
///
/// # Errors
/// `invalid_args` when `title` is absent or blank.
pub fn require_title(title: Option<&str>) -> Result<()> {
    match title {
        Some(t) if !t.trim().is_empty() => Ok(()),
        _ => Err(CoreError::invalid(
            "invalid_args",
            "title is required when creating an artifact",
        )),
    }
}

/// `text` fit to pass [`check_meta_text`] as a field bounded by `max`
/// characters: control characters other than whitespace and hidden format
/// characters ([`crate::text::is_hidden_char`]) dropped, runs of whitespace
/// made one space, trimmed, cut to `max`. `None` when nothing is left. For
/// metadata stored before those checks, sent on to a daemon that makes them.
pub fn fit_meta_text(text: &str, max: usize) -> Option<String> {
    let kept: String = text
        .chars()
        .filter(|&c| c.is_whitespace() || !(c.is_control() || crate::text::is_hidden_char(c)))
        .collect();
    let one: String = kept
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect();
    let one = one.trim_end().to_string();
    (!one.is_empty()).then_some(one)
}

/// A title given on an update: absent keeps the artifact's, and a blank one
/// is refused rather than stored.
///
/// # Errors
/// `invalid_args` when `title` is blank.
pub fn check_update_title(title: Option<&str>) -> Result<()> {
    match title {
        Some(t) if t.trim().is_empty() => {
            Err(CoreError::invalid("invalid_args", "title cannot be blank"))
        }
        _ => Ok(()),
    }
}

/// Checks an artifact's text metadata, as a publish or a metadata edit sets
/// it: `title` at most [`MAX_TITLE_CHARS`] characters, `description`
/// [`MAX_DESCRIPTION_CHARS`] and `icon` [`MAX_ICON_CHARS`], and none holding
/// a control character (line breaks and tabs among them) or a hidden format
/// character ([`crate::text::is_hidden_char`]: bidi controls, zero-width
/// spaces, tags), which would make listings and one-line views read
/// differently from what was stored. Absent fields pass, and so does a
/// blank title, which [`require_title`] refuses on create and
/// [`check_update_title`] on an update.
///
/// # Errors
/// `title_too_long`, `description_too_long` or `icon_too_long` past the
/// bound; `invalid_args` for a refused character.
pub fn check_meta_text(
    title: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
) -> Result<()> {
    for (field, value, max, code) in [
        ("title", title, MAX_TITLE_CHARS, "title_too_long"),
        (
            "description",
            description,
            MAX_DESCRIPTION_CHARS,
            "description_too_long",
        ),
        ("icon", icon, MAX_ICON_CHARS, "icon_too_long"),
    ] {
        let Some(v) = value else { continue };
        if field == "title" && v.trim().is_empty() {
            continue;
        }
        if v.chars().count() > max {
            return Err(CoreError::invalid(
                code,
                format!("{field} exceeds {max} characters"),
            ));
        }
        if v.chars()
            .any(|c| c.is_control() || crate::text::is_hidden_char(c))
        {
            return Err(CoreError::invalid(
                "invalid_args",
                format!("{field} holds a control or hidden format character"),
            ));
        }
    }
    Ok(())
}

/// The declaration a publish makes, from its `capabilities` argument
/// (`None` when omitted or `null`, already validated) and its files:
///
/// - a `clax.json` among them ([`crate::manifest::MANIFEST_PATH`]) is the
///   declaration: its `capabilities` must parse and validate, and an
///   argument must equal it (else 400 `manifest_mismatch`);
/// - an argument without one writes it ([`crate::manifest::render`]) into
///   the files, replacing a removal of it;
/// - a removal of it with no argument clears the declaration to `{}`;
/// - neither keeps the stored declaration (`None`), and the previous
///   version's `clax.json`, if any, carries forward as files do.
///
/// # Errors
/// `invalid_manifest`, `invalid_capabilities` or `manifest_mismatch`.
fn declaration(
    argument: Option<serde_json::Value>,
    files: &mut BTreeMap<String, FileChange>,
) -> Result<Option<serde_json::Value>> {
    use crate::manifest::{MANIFEST_PATH, parse, render};
    match (files.get(MANIFEST_PATH), argument) {
        (Some(FileChange::Put(f)), argument) => {
            let caps = parse(&f.bytes)?;
            crate::capabilities::validate(&caps)?;
            if argument.is_some_and(|a| a != caps) {
                return Err(CoreError::invalid(
                    "manifest_mismatch",
                    "capabilities differs from clax.json's; pass one of them, or the same declaration in both",
                ));
            }
            Ok(Some(caps))
        }
        (_, Some(caps)) => {
            files.insert(
                MANIFEST_PATH.to_string(),
                FileChange::Put(DecodedFile {
                    bytes: render(&caps),
                    content_type: content_type_for(MANIFEST_PATH),
                }),
            );
            Ok(Some(caps))
        }
        (Some(FileChange::Remove), None) => Ok(Some(serde_json::json!({}))),
        (None, None) => Ok(None),
    }
}

/// Checks a publish request: `index.html` present and not removed, every path
/// safe, encodings decodable, size caps, label length and the text metadata
/// ([`check_meta_text`]) respected, an `icon.svg` within the icon rules
/// ([`crate::icon::problem`]), and the declaration resolved from the
/// `capabilities` argument and `clax.json` ([`declaration`]). Base64 content
/// may contain ASCII whitespace (line breaks from encoders), which is ignored.
pub fn validate(req: PublishRequest) -> Result<ValidatedPublish> {
    let (note, note_truncated) = check_declared(
        req.capabilities.as_ref(),
        req.label.as_deref(),
        req.note.as_deref(),
    )?;
    check_meta_text(
        req.title.as_deref(),
        req.description.as_deref(),
        req.icon.as_deref(),
    )?;
    match req.files.get(INDEX) {
        Some(Some(_)) => {}
        _ => {
            return Err(CoreError::invalid(
                "missing_index",
                "index.html is required on every publish",
            ));
        }
    }
    let addresses = req.addresses.clone().unwrap_or_default();
    if addresses.len() > crate::changelog::MAX_ADDRESSES {
        return Err(CoreError::invalid(
            "invalid_args",
            format!("at most {} addresses", crate::changelog::MAX_ADDRESSES),
        ));
    }
    if let Some(bad) = addresses.iter().find(|t| !crate::is_ulid(t)) {
        return Err(CoreError::invalid(
            "invalid_args",
            format!("'{bad}' is not a thread ID"),
        ));
    }
    let mut total: u64 = 0;
    let mut files = BTreeMap::new();
    for (path, input) in req.files {
        check_path(&path)?;
        match input {
            // A file published before its path was reserved can still be removed.
            None => {
                files.insert(path, FileChange::Remove);
            }
            Some(f) => {
                // `_blob/` is refused for a legacy artifact where its row
                // is read (`Store::publish_version`).
                check_path_for(&path, false)?;
                let bytes = match f.encoding {
                    Encoding::Utf8 => f.content.into_bytes(),
                    Encoding::Base64 => base64::engine::general_purpose::STANDARD
                        .decode(
                            f.content
                                .bytes()
                                .filter(|b| !b.is_ascii_whitespace())
                                .collect::<Vec<u8>>(),
                        )
                        .map_err(|_| {
                            CoreError::invalid(
                                "invalid_encoding",
                                format!("'{path}' is not valid base64"),
                            )
                        })?,
                };
                if bytes.len() as u64 > MAX_FILE_BYTES {
                    return Err(CoreError::invalid(
                        "file_too_large",
                        format!("'{path}' exceeds {MAX_FILE_BYTES} bytes"),
                    ));
                }
                total += bytes.len() as u64;
                if total > MAX_BODY_BYTES {
                    return Err(CoreError::invalid(
                        "body_too_large",
                        format!("publish exceeds {MAX_BODY_BYTES} bytes"),
                    ));
                }
                if path == crate::icon::ICON_PATH
                    && let Some(p) = crate::icon::problem(&bytes)
                {
                    return Err(CoreError::InvalidIcon(p));
                }
                let content_type = f.content_type.unwrap_or_else(|| content_type_for(&path));
                files.insert(
                    path,
                    FileChange::Put(DecodedFile {
                        bytes,
                        content_type,
                    }),
                );
            }
        }
    }
    let capabilities = declaration(req.capabilities, &mut files)?;
    Ok(ValidatedPublish {
        title: req.title,
        description: req.description,
        icon: req.icon,
        label: req.label,
        if_version: req.if_version,
        capabilities,
        files,
        note,
        note_truncated,
        addresses,
        working_threads: Vec::new(),
        person: None,
        by_page: false,
        hidden: Vec::new(),
        step: None,
        publish_key: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn page_asset_code(ct: &str, bytes: &[u8]) -> Option<&'static str> {
        match check_page_asset(ct, bytes) {
            Ok(()) => None,
            Err(CoreError::Invalid { code, .. }) => Some(code),
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn a_page_upload_meets_the_contracts_types_sizes_and_bodies() {
        let svg = b"<?xml version=\"1.0\"?>\n<!-- x --><!DOCTYPE svg [<!ENTITY a \"<b>\">]><svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        for (ct, body) in [
            ("image/png", &b"\x89PNG\r\n"[..]),
            ("image/svg+xml", &svg[..]),
            ("text/markdown", b"<!-- stored as given -->\n# hi"),
            ("text/plain", "\u{feff}hello".as_bytes()),
            ("text/css", b"a{}"),
            ("application/json", b"{}"),
        ] {
            assert_eq!(page_asset_code(ct, body), None, "{ct}");
        }
        for (ct, body, want) in [
            ("image/bmp", &b"x"[..], "unsupported_type"),
            ("image/PNG", b"x", "unsupported_type"),
            ("text/html", b"<p>", "unsupported_type"),
            ("text/csv; charset=utf-8", b"a", "unsupported_type"),
            // A PNG that is markup, as a page could pass off a document.
            ("image/png", b"  <html><script>", "unsupported_type"),
            ("image/svg+xml", b"just text", "unsupported_type"),
            ("text/javascript", b"<script>x</script>", "unsupported_type"),
            ("text/css", b"a\0b", "unsupported_type"),
            ("text/csv", b"\xff\xfeA\0", "invalid_request"),
            ("text/plain", b"a\0b", "invalid_request"),
        ] {
            assert_eq!(page_asset_code(ct, body), Some(want), "{ct}");
        }
        let big_svg = [
            b"<svg/>".as_slice(),
            &vec![b' '; PAGE_SVG_MAX_BYTES as usize],
        ]
        .concat();
        assert_eq!(
            page_asset_code("image/svg+xml", &big_svg),
            Some("asset_too_large")
        );
        // A multibyte character where a prolog's opener would be is not a panic.
        assert_eq!(
            page_asset_code("image/svg+xml", "<éé>".as_bytes()),
            Some("unsupported_type")
        );
    }

    fn req(files: &[(&str, Option<FileInput>)]) -> PublishRequest {
        PublishRequest {
            title: Some("T".into()),
            description: None,
            icon: None,
            label: None,
            if_version: None,
            capabilities: None,
            files: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            ..Default::default()
        }
    }
    fn utf8(s: &str) -> Option<FileInput> {
        Some(FileInput {
            content: s.into(),
            encoding: Encoding::Utf8,
            content_type: None,
        })
    }

    #[test]
    fn a_bad_icon_refuses_the_publish() {
        let bad = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 9"><script>x()</script></svg>"#;
        let e = validate(req(&[("index.html", utf8("<p>")), ("icon.svg", utf8(bad))])).unwrap_err();
        assert!(
            matches!(&e, CoreError::InvalidIcon(p) if p.code() == "element"),
            "{e:?}"
        );
        assert!(e.to_string().starts_with("icon.svg uses <script>"), "{e}");
        // Only the app's icon is checked: another .svg, or icon.svg in a
        // folder, is any file; a removal is no icon.
        for (path, file) in [
            ("art.svg", utf8(bad)),
            ("img/icon.svg", utf8(bad)),
            ("icon.svg", None),
        ] {
            validate(req(&[("index.html", utf8("<p>")), (path, file)])).unwrap();
        }
        let good = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 9"/>"#;
        validate(req(&[
            ("index.html", utf8("<p>")),
            ("icon.svg", utf8(good)),
        ]))
        .unwrap();
    }

    #[test]
    fn clax_json_and_the_capabilities_argument_make_one_declaration() {
        use serde_json::json;
        let manifest = |m: &str| req(&[("index.html", utf8("<p>")), ("clax.json", utf8(m))]);
        let p = validate(manifest(r#"{"capabilities": {"db": {}}}"#)).unwrap();
        assert_eq!(p.capabilities, Some(json!({"db": {}})));
        let mut r = manifest(r#"{"capabilities": {"db": {}}}"#);
        r.capabilities = Some(json!({"db": {}}));
        assert!(validate(r).is_ok(), "an equal argument agrees");
        let mut r = manifest(r#"{"capabilities": {"db": {}}}"#);
        r.capabilities = Some(json!({}));
        assert!(matches!(
            validate(r).unwrap_err(),
            CoreError::Invalid {
                code: "manifest_mismatch",
                ..
            }
        ));
        // An argument writes the file, even over its removal.
        for removal in [false, true] {
            let mut r = if removal {
                req(&[("index.html", utf8("<p>")), ("clax.json", None)])
            } else {
                req(&[("index.html", utf8("<p>"))])
            };
            r.capabilities = Some(json!({"user": {}}));
            let p = validate(r).unwrap();
            let Some(FileChange::Put(f)) = p.files.get("clax.json") else {
                panic!("{:?}", p.files.keys());
            };
            assert_eq!(f.bytes, crate::manifest::render(&json!({"user": {}})));
            assert_eq!(f.content_type, "application/json");
        }
        let p = validate(req(&[("index.html", utf8("<p>")), ("clax.json", None)])).unwrap();
        assert_eq!(p.capabilities, Some(json!({})), "a removal clears");
        let p = validate(req(&[("index.html", utf8("<p>"))])).unwrap();
        assert_eq!(p.capabilities, None, "neither keeps");
        assert!(!p.files.contains_key("clax.json"));
    }

    #[test]
    fn index_html_is_required_and_may_not_be_removed() {
        let e = validate(req(&[("app.js", utf8("1"))])).unwrap_err();
        assert!(matches!(
            e,
            CoreError::Invalid {
                code: "missing_index",
                ..
            }
        ));
        let e = validate(req(&[("index.html", None)])).unwrap_err();
        assert!(matches!(
            e,
            CoreError::Invalid {
                code: "missing_index",
                ..
            }
        ));
    }

    #[test]
    fn rejects_unsafe_paths() {
        for bad in [
            "../x.js",
            "/etc/passwd",
            "a\\b.js",
            "a//b.js",
            "a/./b.js",
            "",
            "a/",
            "a/../b",
            "a\u{2028}b.html",
            "a\u{2029}b.html",
        ] {
            let e = validate(req(&[("index.html", utf8("<p>")), (bad, utf8("x"))])).unwrap_err();
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "invalid_path",
                        ..
                    }
                ),
                "{bad}"
            );
        }
        assert!(
            validate(req(&[
                ("index.html", utf8("<p>")),
                ("css/a-b_c.v2.css", utf8("x"))
            ]))
            .is_ok()
        );
    }

    #[test]
    fn decodes_base64_and_rejects_bad_base64() {
        let v = validate(req(&[
            ("index.html", utf8("<p>")),
            (
                "a.bin",
                Some(FileInput {
                    content: "AQID".into(),
                    encoding: Encoding::Base64,
                    content_type: None,
                }),
            ),
        ]))
        .unwrap();
        match &v.files["a.bin"] {
            FileChange::Put(f) => {
                assert_eq!(f.bytes, vec![1, 2, 3]);
                assert_eq!(f.content_type, "application/octet-stream");
            }
            _ => panic!(),
        }
        let e = validate(req(&[
            ("index.html", utf8("<p>")),
            (
                "a.bin",
                Some(FileInput {
                    content: "!!!".into(),
                    encoding: Encoding::Base64,
                    content_type: None,
                }),
            ),
        ]))
        .unwrap_err();
        assert!(matches!(
            e,
            CoreError::Invalid {
                code: "invalid_encoding",
                ..
            }
        ));
    }

    #[test]
    fn content_types_come_from_extension_or_override() {
        let v = validate(req(&[
            ("index.html", utf8("<p>")),
            ("app.js", utf8("1")),
            (
                "data.csv",
                Some(FileInput {
                    content: "a".into(),
                    encoding: Encoding::Utf8,
                    content_type: Some("text/plain".into()),
                }),
            ),
        ]))
        .unwrap();
        let ct = |k: &str| match &v.files[k] {
            FileChange::Put(f) => f.content_type.clone(),
            _ => panic!(),
        };
        assert_eq!(ct("index.html"), "text/html");
        assert_eq!(ct("app.js"), "text/javascript");
        assert_eq!(ct("data.csv"), "text/plain");
    }

    #[test]
    fn enforces_size_caps_and_label_length() {
        let big = "x".repeat(MAX_FILE_BYTES as usize + 1);
        let e = validate(req(&[("index.html", utf8("<p>")), ("big.txt", utf8(&big))])).unwrap_err();
        assert!(matches!(
            e,
            CoreError::Invalid {
                code: "file_too_large",
                ..
            }
        ));
        let mut r = req(&[("index.html", utf8("<p>"))]);
        r.label = Some("x".repeat(61));
        assert!(matches!(
            validate(r).unwrap_err(),
            CoreError::Invalid {
                code: "label_too_long",
                ..
            }
        ));
    }

    #[test]
    fn fitted_metadata_passes_the_checks() {
        let title = format!("a\n\tb\u{202E}\u{200B} {}", "t".repeat(300));
        let t = fit_meta_text(&title, MAX_TITLE_CHARS).unwrap();
        assert!(t.starts_with("a b "), "{t}");
        assert_eq!(t.chars().count(), MAX_TITLE_CHARS);
        let d = fit_meta_text("line one\r\nline two\u{7}", MAX_DESCRIPTION_CHARS).unwrap();
        assert_eq!(d, "line one line two");
        check_meta_text(Some(&t), Some(&d), None).unwrap();
        assert_eq!(fit_meta_text(" \n\u{FEFF}", MAX_ICON_CHARS), None);
    }

    #[test]
    fn a_blank_title_is_refused_on_an_update() {
        for blank in ["", " ", "\n\t"] {
            assert!(matches!(
                check_update_title(Some(blank)),
                Err(CoreError::Invalid {
                    code: "invalid_args",
                    ..
                })
            ));
        }
        assert!(check_update_title(None).is_ok());
        assert!(check_update_title(Some("T")).is_ok());
    }

    #[test]
    fn bounds_title_description_and_icon() {
        let with = |f: &dyn Fn(&mut PublishRequest)| {
            let mut r = req(&[("index.html", utf8("<p>"))]);
            f(&mut r);
            validate(r).map(|_| ()).map_err(|e| match e {
                CoreError::Invalid { code, .. } => code,
                e => panic!("{e:?}"),
            })
        };
        assert_eq!(
            with(&|r| {
                r.title = Some("t".repeat(MAX_TITLE_CHARS));
                r.description = Some("d".repeat(MAX_DESCRIPTION_CHARS));
                r.icon = Some("i".repeat(MAX_ICON_CHARS));
            }),
            Ok(())
        );
        assert_eq!(
            with(&|r| r.title = Some("é".repeat(MAX_TITLE_CHARS + 1))),
            Err("title_too_long")
        );
        assert_eq!(
            with(&|r| r.description = Some("d".repeat(MAX_DESCRIPTION_CHARS + 1))),
            Err("description_too_long")
        );
        assert_eq!(
            with(&|r| r.icon = Some("i".repeat(MAX_ICON_CHARS + 1))),
            Err("icon_too_long")
        );
        for bad in ["a\nb", "\u{202E}txt.exe", "a\u{200B}b", "x\u{E0041}"] {
            assert_eq!(
                with(&|r| r.title = Some(bad.into())),
                Err("invalid_args"),
                "{bad:?}"
            );
            assert_eq!(
                with(&|r| r.description = Some(bad.into())),
                Err("invalid_args")
            );
            assert_eq!(with(&|r| r.icon = Some(bad.into())), Err("invalid_args"));
        }
        // Joiners and direction marks stay: emoji sequences and right-to-left text.
        assert_eq!(
            with(&|r| r.title = Some("\u{1F469}\u{200D}\u{1F4BB} \u{05E9}\u{200F}".into())),
            Ok(())
        );
    }

    #[test]
    fn content_type_uses_the_lowercased_path_extension() {
        assert_eq!(content_type_for("APP.JS"), "text/javascript");
        assert_eq!(content_type_for("lib/Mod.MJS"), "text/javascript");
        assert_eq!(content_type_for("Index.HTML"), "text/html");
        assert_eq!(content_type_for("PHOTO.PNG"), "image/png");
        // A bare name that equals an extension has no extension.
        assert_eq!(content_type_for("js"), "application/octet-stream");
        assert_eq!(content_type_for("dir.js/file"), "application/octet-stream");
    }

    #[test]
    fn base64_ignores_ascii_whitespace() {
        let v = validate(req(&[
            ("index.html", utf8("<p>")),
            (
                "a.bin",
                Some(FileInput {
                    content: "AQID\nBAUG\r\n BwgJ\t".into(),
                    encoding: Encoding::Base64,
                    content_type: None,
                }),
            ),
        ]))
        .unwrap();
        match &v.files["a.bin"] {
            FileChange::Put(f) => assert_eq!(f.bytes, (1..=9).collect::<Vec<u8>>()),
            _ => panic!(),
        }
    }

    #[test]
    fn total_size_cap_is_enforced() {
        let chunk = "x".repeat(MAX_FILE_BYTES as usize);
        let n = (MAX_BODY_BYTES / MAX_FILE_BYTES) as usize;
        let names: Vec<String> = (0..n).map(|i| format!("f{i}.txt")).collect();
        let mut files: Vec<(&str, Option<FileInput>)> = vec![("index.html", utf8("<p>"))];
        files.extend(names.iter().map(|k| (k.as_str(), utf8(&chunk))));
        let e = validate(req(&files)).unwrap_err();
        assert!(matches!(
            e,
            CoreError::Invalid {
                code: "body_too_large",
                ..
            }
        ));
    }

    #[test]
    fn removals_pass_through() {
        let v = validate(req(&[("index.html", utf8("<p>")), ("old.css", None)])).unwrap();
        assert!(matches!(v.files["old.css"], FileChange::Remove));
    }

    #[test]
    fn html_title_matches_the_contract_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/pi/test/fixtures/contract.json");
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read contract.json")).unwrap();
        let cases = fixture["html_title"].as_array().unwrap();
        assert!(!cases.is_empty());
        for c in cases {
            let html = c["html"].as_str().unwrap();
            assert_eq!(html_title(html).as_deref(), c["title"].as_str(), "{html:?}");
        }
        assert_eq!(MAX_DERIVED_TITLE_CHARS, 200);
    }

    #[test]
    fn invalid_capabilities_are_refused_on_publish() {
        let mut r = req(&[(
            "index.html",
            Some(FileInput {
                content: "<p>".into(),
                encoding: Encoding::Utf8,
                content_type: None,
            }),
        )]);
        r.capabilities = Some(serde_json::json!({"db": {"rules": [{"path": "a/{self}/b"}]}}));
        assert!(matches!(
            validate(r).unwrap_err(),
            CoreError::Invalid {
                code: "invalid_capabilities",
                ..
            }
        ));
    }
}

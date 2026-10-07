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
    /// The page itself published through the shell (false from
    /// [`validate`]; the caller sets it). Recorded, not stored.
    pub by_page: bool,
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
/// other entity is kept as written); runs of whitespace become one space; the
/// result is trimmed and cut to [`MAX_DERIVED_TITLE_CHARS`] characters. `None`
/// when there is no closed `<title>` element or its text is empty.
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
    let text = decode_basic_entities(&html[body_start..body_end]);
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

/// Checks a publish request: `index.html` present and not removed, every path
/// safe, encodings decodable, size caps and label length respected. Base64
/// content may contain ASCII whitespace (line breaks from encoders), which is
/// ignored.
pub fn validate(req: PublishRequest) -> Result<ValidatedPublish> {
    if let Some(caps) = &req.capabilities {
        crate::capabilities::validate(caps)?;
    }
    if let Some(label) = &req.label
        && label.chars().count() > MAX_LABEL_CHARS
    {
        return Err(CoreError::invalid(
            "label_too_long",
            format!("label exceeds {MAX_LABEL_CHARS} characters"),
        ));
    }
    match req.files.get(INDEX) {
        Some(Some(_)) => {}
        _ => {
            return Err(CoreError::invalid(
                "missing_index",
                "index.html is required on every publish",
            ));
        }
    }
    let (note, note_truncated) = match req.note.as_deref() {
        Some(n) => crate::working::clean_line(n, crate::changelog::MAX_NOTE_CHARS),
        None => (None, false),
    };
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
            None => {
                files.insert(path, FileChange::Remove);
            }
            Some(f) => {
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
    Ok(ValidatedPublish {
        title: req.title,
        description: req.description,
        icon: req.icon,
        label: req.label,
        if_version: req.if_version,
        capabilities: req.capabilities,
        files,
        note,
        note_truncated,
        addresses,
        working_threads: Vec::new(),
        by_page: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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

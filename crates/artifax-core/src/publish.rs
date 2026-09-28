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
}

/// Accepts a relative path of one or more non-empty segments, with no `.` or
/// `..` segments, no control characters, and forward slashes only.
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
        if seg.is_empty() || seg == "." || seg == ".." || seg.chars().any(|c| c.is_control()) {
            return Err(bad());
        }
    }
    Ok(())
}

/// Content type inferred from the file extension; JavaScript is always
/// `text/javascript`, unknown extensions are `application/octet-stream`.
pub fn content_type_for(path: &str) -> String {
    match path.rsplit('.').next() {
        Some("js") | Some("mjs") => "text/javascript".to_string(),
        _ => mime_guess::from_path(path)
            .first_raw()
            .unwrap_or("application/octet-stream")
            .to_string(),
    }
}

/// Checks a publish request: `index.html` present and not removed, every path
/// safe, encodings decodable, size caps and label length respected.
pub fn validate(req: PublishRequest) -> Result<ValidatedPublish> {
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
                        .decode(f.content.as_bytes())
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
    fn removals_pass_through() {
        let v = validate(req(&[("index.html", utf8("<p>")), ("old.css", None)])).unwrap();
        assert!(matches!(v.files["old.css"], FileChange::Remove));
    }
}

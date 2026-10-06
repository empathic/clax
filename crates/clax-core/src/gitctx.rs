//! The git state of an agent's working directory (spec
//! 2026-10-06-toolpath-audit-design §9), as the agent side captures it and
//! sends it in the `x-clax-git` header.
//!
//! The header value is base64url JSON of at most [`MAX_HEADER_BYTES`]. It is
//! either a [`GitContext`] or `{"git_capture":"<outcome>"}`, naming why there
//! is no context. The daemon records what the agent side reported after
//! checking its shape; a header that fails the check is recorded as the
//! outcome `invalid` and never fails the request.

use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The largest header value accepted, in bytes of the encoded value.
pub const MAX_HEADER_BYTES: usize = 2048;

/// A captured working-directory state. Contents, diff paths and file names
/// are never part of it: only the diff's hash and size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitContext {
    /// `git rev-parse --show-toplevel`: an absolute path.
    pub repo_root: String,
    /// The upstream remote's name, else `origin`, else the first remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// That remote's URL, passed through [`sanitize_remote`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    /// The checked-out branch; absent when HEAD is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// HEAD's object name, 40 (or, in a SHA-256 repository, 64) lowercase
    /// hex digits; absent on an unborn branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// True when `git status --porcelain` prints anything.
    pub dirty: bool,
    /// `sha256:<hex>` of `git diff HEAD --binary …`; absent when there is no
    /// diff to tracked files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_sha256: Option<String>,
    /// The size of that diff, in bytes (up to the cap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_bytes: Option<u64>,
    /// True when the diff passed the cap and only its prefix was hashed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub diff_truncated: bool,
    /// The count of untracked, unignored paths.
    pub untracked: u64,
    /// RFC 3339 time of the capture, from the agent side's clock.
    pub captured_at: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl GitContext {
    /// Checks the shape the daemon accepts: an absolute `repo_root`; no
    /// control characters anywhere; a well-formed `head`, `diff_sha256` and
    /// `captured_at`; a `branch` that git's refname rules accept; a
    /// `remote_url` that [`sanitize_remote`] leaves unchanged; and diff
    /// fields and untracked paths only on a dirty tree.
    pub fn validate(&self) -> Result<(), String> {
        if !std::path::Path::new(&self.repo_root).is_absolute() {
            return Err("repo_root is not an absolute path".into());
        }
        text("repo_root", &self.repo_root)?;
        for (name, v) in [
            ("remote", &self.remote),
            ("remote_url", &self.remote_url),
            ("branch", &self.branch),
        ] {
            if let Some(v) = v {
                text(name, v)?;
            }
        }
        if let Some(b) = &self.branch
            && !is_branch_name(b)
        {
            return Err("branch is not a valid git branch name".into());
        }
        if let Some(u) = &self.remote_url
            && sanitize_remote(u) != *u
        {
            return Err("remote_url carries a credential, query or fragment".into());
        }
        if let Some(h) = &self.head
            && !((h.len() == 40 || h.len() == 64) && is_lower_hex(h))
        {
            return Err("head is not a 40- or 64-digit lowercase hex object name".into());
        }
        if let Some(d) = &self.diff_sha256 {
            if !is_sha256_ref(d) {
                return Err("diff_sha256 is not sha256:<64 lowercase hex>".into());
            }
            if !self.dirty {
                return Err("diff_sha256 on a clean tree".into());
            }
        } else if self.diff_bytes.is_some() || self.diff_truncated {
            return Err("diff_bytes or diff_truncated without diff_sha256".into());
        }
        if !self.dirty && self.untracked > 0 {
            return Err("untracked paths on a clean tree".into());
        }
        if !is_rfc3339(&self.captured_at) {
            return Err("captured_at is not RFC 3339".into());
        }
        Ok(())
    }
}

/// Fails when `v` is empty or holds a control character or an invisible
/// format character (bidi controls, zero-width characters, tags), which
/// could make rendered output read differently from what was stored.
pub(crate) fn text(name: &str, v: &str) -> Result<(), String> {
    if v.is_empty() {
        return Err(format!("{name} is empty"));
    }
    if v.chars().any(|c| c.is_control() || is_format_char(c)) {
        return Err(format!("{name} holds a control or format character"));
    }
    Ok(())
}

/// Unicode format (category Cf) characters, the bidi controls among them.
fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// True when `git check-ref-format --branch` would accept `b`: no space,
/// control character or any of `~^:?*[\`; no `..`, `@{` or `//`; not `@`;
/// no leading `-` or `/`; no trailing `/` or `.`; and no component that
/// starts with `.` or ends with `.lock`.
fn is_branch_name(b: &str) -> bool {
    !(b.is_empty()
        || b == "@"
        || b.starts_with(['-', '/'])
        || b.ends_with(['/', '.'])
        || b.contains("..")
        || b.contains("@{")
        || b.contains("//")
        || b.chars()
            .any(|c| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
        || b.split('/')
            .any(|part| part.starts_with('.') || part.ends_with(".lock")))
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// True when `s` is `sha256:` and 64 lowercase hex digits.
pub(crate) fn is_sha256_ref(s: &str) -> bool {
    s.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && is_lower_hex(h))
}

pub(crate) fn is_rfc3339(s: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
}

/// Base64url without padding on encode; padding optional on decode.
const B64: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    base64::engine::GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
);

/// `value` as a header: base64url JSON, or an error past [`MAX_HEADER_BYTES`].
pub(crate) fn encode_json<T: Serialize>(value: &T) -> Result<String, String> {
    let json = serde_json::to_vec(value).map_err(|_| "header does not serialize".to_string())?;
    let h = B64.encode(json);
    if h.len() > MAX_HEADER_BYTES {
        return Err(format!("header is over {MAX_HEADER_BYTES} bytes"));
    }
    Ok(h)
}

/// Reads a header written by [`encode_json`]. The size is checked before
/// anything is decoded. Errors are fixed messages that never echo the input.
pub(crate) fn decode_json<T: DeserializeOwned>(value: &str) -> Result<T, String> {
    if value.len() > MAX_HEADER_BYTES {
        return Err(format!("header is over {MAX_HEADER_BYTES} bytes"));
    }
    let bytes = B64
        .decode(value)
        .map_err(|_| "header is not base64url".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "header does not hold the expected JSON".to_string())
}

/// The header form of an outcome without a context.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureOnly {
    git_capture: String,
}

/// The two header forms, told apart by their fields.
#[derive(Deserialize)]
#[serde(untagged)]
enum Header {
    Capture(CaptureOnly),
    Context(Box<GitContext>),
}

/// The git half of a request's audit context.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GitField {
    /// The agent side captured this context.
    Ok(GitContext),
    /// The agent side tried and could not, or sent a header that failed the
    /// check: one of [`CAPTURE_OUTCOMES`].
    Capture(&'static str),
    /// No header: the request carries no git context.
    #[default]
    Absent,
}

/// The outcomes recorded as `git_capture` when there is no context.
pub const CAPTURE_OUTCOMES: [&str; 5] =
    ["not-a-repo", "timeout", "unavailable", "no-cwd", "invalid"];

impl GitField {
    /// The context, when there is one.
    pub fn context(&self) -> Option<&GitContext> {
        match self {
            GitField::Ok(c) => Some(c),
            _ => None,
        }
    }

    /// The `git_capture` value recorded: `ok` with a context, the outcome
    /// without one, nothing when absent.
    pub fn capture(&self) -> Option<&'static str> {
        match self {
            GitField::Ok(_) => Some("ok"),
            GitField::Capture(o) => Some(o),
            GitField::Absent => None,
        }
    }
}

/// The `x-clax-git` header value for `field`, or `None` for
/// [`GitField::Absent`]. A context that fails [`GitContext::validate`], or
/// whose encoding would pass [`MAX_HEADER_BYTES`], is sent as the outcome
/// `invalid`, so nothing the daemon would refuse leaves the agent side.
pub fn encode_header(field: &GitField) -> Option<String> {
    let outcome = |o: &str| {
        encode_json(&CaptureOnly {
            git_capture: o.to_owned(),
        })
        .ok()
    };
    match field {
        GitField::Ok(c) if c.validate().is_ok() => {
            encode_json(c).ok().or_else(|| outcome("invalid"))
        }
        GitField::Ok(_) => outcome("invalid"),
        GitField::Capture(o) => outcome(o),
        GitField::Absent => None,
    }
}

/// Reads an `x-clax-git` header value. Anything that is not base64url JSON of
/// at most [`MAX_HEADER_BYTES`], holding either a context that passes
/// [`GitContext::validate`] or a known outcome, is [`GitField::Capture`]
/// `("invalid")`.
pub fn decode_header(value: &str) -> GitField {
    const INVALID: GitField = GitField::Capture("invalid");
    match decode_json::<Header>(value) {
        Ok(Header::Capture(c)) => CAPTURE_OUTCOMES
            .iter()
            .find(|o| **o == c.git_capture)
            .map_or(INVALID, |o| GitField::Capture(o)),
        Ok(Header::Context(c)) if c.validate().is_ok() => GitField::Ok(*c),
        _ => INVALID,
    }
}

/// A remote URL with any credential removed, and with the query and fragment
/// dropped. For `ssh`-family schemes a bare user (`ssh://git@host/x`) is kept
/// and a `user:password@` is removed; for every other scheme all userinfo is
/// removed, because hosts accept a bare token as the user. Userinfo ends at
/// the last `@` before the first `/` that follows any `@`, so a password
/// holding an unencoded `@`, `/`, `?` or `#` is still removed. The scp form
/// `git@host:owner/repo.git` and local paths are kept as they are.
pub fn sanitize_remote(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    // Userinfo may hold an unencoded `/`, `?` or `#`, so it ends at the last
    // `@` before the first `/` that follows the first `@`.
    let (userinfo, after) = match rest.find('@') {
        Some(first) => {
            let limit = rest[first..].find('/').map_or(rest.len(), |i| first + i);
            let at = rest[..limit].rfind('@').unwrap_or(first);
            (Some(&rest[..at]), &rest[at + 1..])
        }
        None => (None, rest),
    };
    let host_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (host, path) = after.split_at(host_end);
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    let ssh = scheme.split('+').any(|s| s.eq_ignore_ascii_case("ssh"));
    match userinfo {
        Some(user) if ssh && !user.contains([':', '/', '?', '#']) => {
            format!("{scheme}://{user}@{host}{path}")
        }
        _ => format!("{scheme}://{host}{path}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> GitContext {
        GitContext {
            repo_root: "/Users/alex/work/app".into(),
            remote: Some("origin".into()),
            remote_url: Some("https://github.com/empathic/app.git".into()),
            branch: Some("feat/settings".into()),
            head: Some("9c1e5d2b0a7f4e3c8d6b1a2f3e4d5c6b7a8f9e0d".into()),
            dirty: true,
            diff_sha256: Some(format!("sha256:{}", "ab".repeat(32))),
            diff_bytes: Some(18234),
            diff_truncated: false,
            untracked: 2,
            captured_at: "2026-10-06T14:03:11.512Z".into(),
        }
    }

    #[test]
    fn git_header_roundtrip() {
        let field = GitField::Ok(sample());
        let h = encode_header(&field).unwrap();
        assert!(
            h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_eq!(decode_header(&h), field);

        let bare = GitField::Ok(GitContext {
            remote: None,
            remote_url: None,
            branch: None,
            head: None,
            dirty: false,
            diff_sha256: None,
            diff_bytes: None,
            untracked: 0,
            ..sample()
        });
        assert_eq!(decode_header(&encode_header(&bare).unwrap()), bare);

        for o in ["not-a-repo", "timeout", "unavailable", "no-cwd"] {
            let f = GitField::Capture(o);
            assert_eq!(decode_header(&encode_header(&f).unwrap()), f);
        }
        assert_eq!(encode_header(&GitField::Absent), None);

        // Padded base64url is accepted too.
        use base64::Engine as _;
        let json = serde_json::to_vec(&sample()).unwrap();
        let padded = base64::engine::general_purpose::URL_SAFE.encode(json);
        assert_eq!(decode_header(&padded), GitField::Ok(sample()));
    }

    #[test]
    fn malformed_headers_are_invalid() {
        use base64::Engine as _;
        let enc = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        let invalid = GitField::Capture("invalid");
        for bad in [
            "".to_string(),
            "not base64!".to_string(),
            enc("not json"),
            enc("[]"),
            enc(r#"{"git_capture":"ok"}"#),
            enc(r#"{"git_capture":"exploded"}"#),
            enc(r#"{"git_capture":"timeout","x":1}"#),
            enc(r#"{"repo_root":"/r","dirty":false,"untracked":0}"#),
        ] {
            assert_eq!(decode_header(&bad), invalid, "{bad}");
        }
        // Unknown fields are refused, so nothing rides along unchecked.
        let mut v = serde_json::to_value(sample()).unwrap();
        v["diff"] = "secret contents".into();
        assert_eq!(decode_header(&enc(&v.to_string())), invalid);
    }

    #[test]
    fn oversized_header_is_invalid() {
        let mut big = sample();
        big.repo_root = format!("/{}", "x".repeat(MAX_HEADER_BYTES));
        assert_eq!(big.validate(), Ok(()));
        let field = GitField::Ok(big.clone());
        // Encoding degrades to the `invalid` outcome rather than sending it.
        let h = encode_header(&field).unwrap();
        assert!(h.len() <= MAX_HEADER_BYTES);
        assert_eq!(decode_header(&h), GitField::Capture("invalid"));
        // A raw oversized value is refused before decoding.
        use base64::Engine as _;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&big).unwrap());
        assert!(raw.len() > MAX_HEADER_BYTES);
        assert_eq!(decode_header(&raw), GitField::Capture("invalid"));
        // Exactly at the limit is accepted.
        let mut fit = sample();
        let base = encode_header(&GitField::Ok(fit.clone())).unwrap().len();
        // Each 3 extra JSON bytes add 4 encoded bytes.
        let room = (MAX_HEADER_BYTES - base) / 4 * 3;
        fit.repo_root.push_str(&"y".repeat(room));
        let h = encode_header(&GitField::Ok(fit.clone())).unwrap();
        assert!(h.len() <= MAX_HEADER_BYTES && h.len() > MAX_HEADER_BYTES - 4);
        assert_eq!(decode_header(&h), GitField::Ok(fit));
    }

    #[test]
    fn userinfo_is_stripped() {
        for (input, want) in [
            (
                "https://u:tok@github.com/o/r.git?x=1#f",
                "https://github.com/o/r.git",
            ),
            (
                "https://ghp_token@github.com/o/r.git",
                "https://github.com/o/r.git",
            ),
            ("https://github.com/o/r.git", "https://github.com/o/r.git"),
            ("http://host:8080/o/r#frag", "http://host:8080/o/r"),
            (
                "git@github.com:owner/repo.git",
                "git@github.com:owner/repo.git",
            ),
            ("ssh://git@host/x", "ssh://git@host/x"),
            ("ssh://u:p@host/x", "ssh://host/x"),
            ("ssh://git@host:2222/x?q", "ssh://git@host:2222/x"),
            ("git+ssh://u:p@host/x", "git+ssh://host/x"),
            ("ssh://u:p@host", "ssh://host"),
            ("https://u:p@host?x", "https://host"),
            ("/srv/git/app.git", "/srv/git/app.git"),
            ("file:///srv/git/app.git", "file:///srv/git/app.git"),
            ("../app", "../app"),
        ] {
            assert_eq!(sanitize_remote(input), want, "{input}");
        }
    }

    #[test]
    fn validate_rejects_bad_head() {
        assert_eq!(sample().validate(), Ok(()));
        let sha256_head = GitContext {
            head: Some("0123456789abcdef".repeat(4)),
            ..sample()
        };
        assert_eq!(sha256_head.validate(), Ok(()));
        for head in [
            "",
            "abc1234",
            "9C1E5D2B0A7F4E3C8D6B1A2F3E4D5C6B7A8F9E0D",
            "9c1e5d2b0a7f4e3c8d6b1a2f3e4d5c6b7a8f9e0d0",
            "9c1e5d2b0a7f4e3c8d6b1a2f3e4d5c6b7a8f9e0g",
            "HEAD",
        ] {
            let c = GitContext {
                head: Some(head.into()),
                ..sample()
            };
            assert!(c.validate().is_err(), "{head:?}");
            assert_eq!(
                decode_header(&encode_json(&c).unwrap()),
                GitField::Capture("invalid")
            );
        }
    }

    #[test]
    fn validate_rejects_other_bad_fields() {
        let cases: Vec<(&str, GitContext)> = vec![
            (
                "relative root",
                GitContext {
                    repo_root: "work/app".into(),
                    ..sample()
                },
            ),
            (
                "empty root",
                GitContext {
                    repo_root: "".into(),
                    ..sample()
                },
            ),
            (
                "control in branch",
                GitContext {
                    branch: Some("a\nb".into()),
                    ..sample()
                },
            ),
            (
                "empty branch",
                GitContext {
                    branch: Some("".into()),
                    ..sample()
                },
            ),
            (
                "empty remote",
                GitContext {
                    remote: Some("".into()),
                    ..sample()
                },
            ),
            (
                "credential in url",
                GitContext {
                    remote_url: Some("https://u:t@h/r".into()),
                    ..sample()
                },
            ),
            (
                "bad diff hash",
                GitContext {
                    diff_sha256: Some("sha256:xyz".into()),
                    ..sample()
                },
            ),
            (
                "unprefixed diff hash",
                GitContext {
                    diff_sha256: Some("ab".repeat(32)),
                    ..sample()
                },
            ),
            (
                "diff on clean tree",
                GitContext {
                    dirty: false,
                    untracked: 0,
                    ..sample()
                },
            ),
            (
                "bytes without hash",
                GitContext {
                    diff_sha256: None,
                    ..sample()
                },
            ),
            (
                "bad time",
                GitContext {
                    captured_at: "yesterday".into(),
                    ..sample()
                },
            ),
        ];
        for (name, c) in cases {
            assert!(c.validate().is_err(), "{name}");
        }
        // Dirty with only untracked files: no diff hash, which is fine.
        let untracked_only = GitContext {
            diff_sha256: None,
            diff_bytes: None,
            ..sample()
        };
        assert_eq!(untracked_only.validate(), Ok(()));
    }

    #[test]
    fn encode_refuses_to_send_an_invalid_context() {
        let leaky = GitContext {
            remote_url: Some("https://u:tok@github.com/o/r.git".into()),
            ..sample()
        };
        let h = encode_header(&GitField::Ok(leaky)).unwrap();
        assert_eq!(decode_header(&h), GitField::Capture("invalid"));
        use base64::Engine as _;
        let sent = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&h)
            .unwrap();
        assert_eq!(sent, br#"{"git_capture":"invalid"}"#);
    }

    #[test]
    fn userinfo_with_delimiters_is_stripped() {
        for (input, want) in [
            ("https://u:p#x@host/r", "https://host/r"),
            ("https://u:p?x@host/r", "https://host/r"),
            ("https://u:p/x@host/r", "https://host/r"),
            ("https://u:p@w@host/r", "https://host/r"),
            (
                "https://x-access-token:ghs_tok@github.com/o/r.git",
                "https://github.com/o/r.git",
            ),
            ("ssh://u:p#x@host/r", "ssh://host/r"),
            ("ssh://u:p@w@host/r", "ssh://host/r"),
            ("https://u:p@host/path@v1", "https://host/path@v1"),
            ("https://host/r.git?x=1#f", "https://host/r.git"),
        ] {
            assert_eq!(sanitize_remote(input), want, "{input}");
        }
    }

    #[test]
    fn validate_rejects_format_characters_and_bad_refnames() {
        for c in [
            '\u{200E}',
            '\u{200F}',
            '\u{202A}',
            '\u{202E}',
            '\u{2066}',
            '\u{2069}',
            '\u{FEFF}',
            '\u{200B}',
            '\u{061C}',
            '\u{E0041}',
        ] {
            let b = GitContext {
                branch: Some(format!("main{c}")),
                ..sample()
            };
            assert!(b.validate().is_err(), "branch with {:04X}", c as u32);
            let r = GitContext {
                remote: Some(format!("or{c}igin")),
                ..sample()
            };
            assert!(r.validate().is_err(), "remote with {:04X}", c as u32);
            let u = GitContext {
                remote_url: Some(format!("https://github.com/o/r{c}.git")),
                ..sample()
            };
            assert!(u.validate().is_err(), "remote_url with {:04X}", c as u32);
        }
        for bad in [
            "a b",
            "a..b",
            "a~1",
            "a^",
            "a:b",
            "a?",
            "a*",
            "a[b",
            "a\\b",
            "-a",
            "/a",
            "a/",
            "a//b",
            ".a",
            "a/.b",
            "a.",
            "a.lock",
            "a/b.lock/c",
            "a@{1}",
            "@",
        ] {
            let b = GitContext {
                branch: Some(bad.into()),
                ..sample()
            };
            assert!(b.validate().is_err(), "{bad:?}");
        }
        for good in [
            "main",
            "feat/settings",
            "release-1.2",
            "a@b",
            "user/x_y",
            "v1.0/rc",
        ] {
            let b = GitContext {
                branch: Some(good.into()),
                ..sample()
            };
            assert_eq!(b.validate(), Ok(()), "{good:?}");
        }
    }

    #[test]
    fn validate_rejects_untracked_on_clean_tree() {
        let c = GitContext {
            dirty: false,
            diff_sha256: None,
            diff_bytes: None,
            untracked: 1,
            ..sample()
        };
        assert!(c.validate().is_err());
        assert_eq!(GitContext { untracked: 0, ..c }.validate(), Ok(()));
    }
}

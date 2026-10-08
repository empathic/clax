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
    /// True when the diff was not hashed because its blobs may have to be
    /// fetched: a partial clone whose objects git could not read without
    /// fetching, or any partial clone under a git too old to refuse lazy
    /// fetches (before 2.44).
    #[serde(default, skip_serializing_if = "is_false")]
    pub diff_unavailable: bool,
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
            if self.diff_unavailable {
                return Err("diff_sha256 with diff_unavailable".into());
            }
        } else if self.diff_unavailable && !self.dirty {
            return Err("diff_unavailable on a clean tree".into());
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

/// How long the agent side lets a capture run (spec L10): past it, every
/// git child is killed and the outcome is `timeout`.
pub const CAPTURE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(300);

/// The flags of the diff hashed (spec §9.1), after `diff HEAD` (or `diff
/// --cached` on an unborn branch). Each pins what the user's configuration
/// could otherwise change, so the same tree hashes the same everywhere.
pub const DIFF_FLAGS: [&str; 16] = [
    "--binary",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--full-index",
    "--no-relative",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--no-renames",
    "--diff-algorithm=myers",
    "--indent-heuristic",
    "--unified=3",
    "--inter-hunk-context=0",
    "-O/dev/null",
    "--ignore-submodules=dirty",
    "--no-color-moved",
];

/// The most diff output hashed; past it the diff is `diff_truncated`.
pub const DIFF_CAP: u64 = 64 << 20;

/// The git executable named `git` on `path` (a `PATH` value), when there
/// is one.
pub fn find_git(path: Option<&std::ffi::OsStr>) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path?)
        .filter(|d| d.is_absolute())
        .map(|d| d.join("git"))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// The oldest git capture runs: 2.44, the first to honour
/// `GIT_NO_LAZY_FETCH`. Older gits silently ignore the safeguards capture
/// relies on (`GIT_CONFIG_COUNT`, which carries every pin, before 2.31;
/// `GIT_NO_LAZY_FETCH`, which keeps `status` and `diff` from fetching in a
/// partial clone, before 2.44), so under them nothing runs.
pub const MIN_GIT: (u32, u32) = (2, 44);

/// How long [`Git::probe`] waits for `git version`.
pub const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// A git executable and whether capture may run it: its `git version`,
/// read once, names [`MIN_GIT`] or later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Git {
    path: std::path::PathBuf,
    supported: bool,
}

impl Git {
    /// Reads `path`'s `git version` (in the capture environment, outside any
    /// repository, within [`PROBE_DEADLINE`]). A git that cannot be run,
    /// does not answer in time, or names an unreadable or older version is
    /// unsupported, and every capture with it is `unavailable`.
    pub fn probe(path: &std::path::Path) -> Git {
        let runner = capture_impl::Runner::new(path, std::path::Path::new("/"));
        let line = capture_impl::version(&runner, std::time::Instant::now() + PROBE_DEADLINE);
        runner.cancel();
        Git {
            path: path.to_path_buf(),
            supported: line.as_deref().is_some_and(version_supported),
        }
    }

    /// Whether capture runs this git.
    pub fn supported(&self) -> bool {
        self.supported
    }

    /// `path`, trusted without a probe: for tests whose fake git answers
    /// only the commands under test.
    #[cfg(test)]
    pub(crate) fn assume_supported(path: &std::path::Path) -> Git {
        Git {
            path: path.to_path_buf(),
            supported: true,
        }
    }
}

/// Whether `git version` output `line` names [`MIN_GIT`] or later.
pub fn version_supported(line: &str) -> bool {
    let mut parts = line
        .strip_prefix("git version ")
        .unwrap_or("")
        .split(|c: char| !c.is_ascii_digit())
        .map(|p| p.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(major), Some(minor)) => (major, minor) >= MIN_GIT,
        _ => false,
    }
}

/// Captures the git state of `cwd` with the executable `git`, by spec §9.1
/// and §9.2, finishing by `deadline` (normally [`CAPTURE_DEADLINE`] from
/// now). It never fails: without a context it is the outcome
///
/// - `no-cwd` when `cwd` is empty or not a directory;
/// - `unavailable` when `git` is older than [`MIN_GIT`] (then nothing is
///   run), cannot be run, or a git command after the first fails;
/// - `not-a-repo` when `git rev-parse --show-toplevel` fails (as it does
///   for a repository another user owns: `safe.directory` is never passed);
/// - `timeout` when `deadline` passes first: every git child still running
///   is killed;
/// - `invalid` when what git reported is not UTF-8 or fails
///   [`GitContext::validate`].
///
/// No command runs a program the repository or the user configured, writes
/// the repository or fetches. Every command runs with `-C <cwd>`, no
/// standard input, its standard error discarded, and an environment cleared
/// down to `PATH`, `HOME`, `XDG_CONFIG_HOME` and `TMPDIR` plus
/// `GIT_OPTIONAL_LOCKS=0 GIT_TERMINAL_PROMPT=0 LC_ALL=C GIT_PAGER=cat
/// GIT_NO_LAZY_FETCH=1`. Configuration is pinned through
/// `GIT_CONFIG_COUNT`: no fsmonitor, `core.hooksPath=/dev/null`,
/// `diff.autoRefreshIndex=false` (a refresh would take `index.lock` and
/// write the index), `core.quotePath=true`, `diff.suppressBlankEmpty=false`
/// and, for every filter driver the configuration names (read first with
/// `git config`, which runs nothing), an empty `clean`, `smudge` and
/// `process` and `required=false`. Diffs pass `--no-ext-diff
/// --no-textconv`, and submodules are compared only by the commit recorded
/// for them (checking their work trees would run git inside them). In a
/// partial clone, a diff that would need a missing object fails rather than
/// fetch, and is `diff_unavailable`. Only hashes and counts are kept: diff
/// contents and file names never leave this function.
pub fn capture(cwd: &std::path::Path, deadline: std::time::Instant, git: &Git) -> GitField {
    if cwd.as_os_str().is_empty() || !cwd.is_dir() {
        return GitField::Capture("no-cwd");
    }
    if !git.supported {
        return GitField::Capture("unavailable");
    }
    let runner = capture_impl::Runner::new(&git.path, cwd);
    let outcome = capture_impl::run(&runner, deadline);
    runner.cancel();
    match outcome {
        Ok(ctx) if ctx.validate().is_ok() => GitField::Ok(ctx),
        Ok(_) => GitField::Capture("invalid"),
        Err(o) => GitField::Capture(o),
    }
}

mod capture_impl {
    use super::{DIFF_CAP, GitContext, sanitize_remote};
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    type Outcome = &'static str;

    /// The variables kept from this process's environment; every other is
    /// cleared, so nothing (`GIT_DIR`, `GIT_CONFIG_*`, `GIT_EXEC_PATH`,
    /// `GIT_TRACE*`, …) points git elsewhere or injects configuration.
    const KEPT_VARS: [&str; 4] = ["PATH", "HOME", "XDG_CONFIG_HOME", "TMPDIR"];

    /// Configuration every command runs with.
    /// Configuration every command runs with: no fsmonitor, no hooks, no
    /// index refresh (which would take `index.lock`, write the index and
    /// run `post-index-change`), and the diff settings no flag pins.
    const BASE_CONFIG: [(&str, &str); 5] = [
        ("core.fsmonitor", "false"),
        ("core.hooksPath", "/dev/null"),
        ("diff.autoRefreshIndex", "false"),
        ("core.quotePath", "true"),
        ("diff.suppressBlankEmpty", "false"),
    ];

    /// The most filter drivers neutralized; a configuration naming more is
    /// refused (`unavailable`) rather than risk running one.
    const MAX_FILTERS: usize = 64;

    #[derive(Default)]
    struct Children {
        cancelled: bool,
        running: Vec<Arc<Mutex<Child>>>,
    }

    /// Runs git commands in one working directory, and kills them all on
    /// [`Runner::cancel`].
    #[derive(Clone)]
    pub(super) struct Runner {
        git: PathBuf,
        cwd: PathBuf,
        config: Arc<Vec<(String, String)>>,
        children: Arc<Mutex<Children>>,
    }

    /// A finished command: whether it exited 0, and its standard output.
    struct Out {
        ok: bool,
        stdout: Vec<u8>,
    }

    impl Out {
        /// The output's first line, when the command succeeded and printed
        /// one; `invalid` when it is not UTF-8.
        fn line(&self) -> Result<Option<String>, Outcome> {
            if !self.ok {
                return Ok(None);
            }
            let s = std::str::from_utf8(&self.stdout).map_err(|_| "invalid")?;
            let line = s.strip_suffix('\n').unwrap_or(s);
            let line = line.strip_suffix('\r').unwrap_or(line);
            Ok((!line.is_empty()).then(|| line.to_string()))
        }
    }

    /// A diff's hash and size.
    struct Diff {
        sha256: String,
        bytes: u64,
        truncated: bool,
    }

    impl Runner {
        pub(super) fn new(git: &Path, cwd: &Path) -> Runner {
            Runner {
                git: git.to_path_buf(),
                cwd: cwd.to_path_buf(),
                config: Arc::new(
                    BASE_CONFIG
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                ),
                children: Arc::default(),
            }
        }

        /// This runner with every filter driver in `drivers` turned off.
        fn without_filters(&self, drivers: &[String]) -> Runner {
            let mut config = (*self.config).clone();
            for d in drivers {
                for (var, value) in [
                    ("clean", ""),
                    ("smudge", ""),
                    ("process", ""),
                    ("required", "false"),
                ] {
                    config.push((format!("filter.{d}.{var}"), value.to_string()));
                }
            }
            Runner {
                config: Arc::new(config),
                ..self.clone()
            }
        }

        /// Kills every child still running, and refuses to start more.
        pub(super) fn cancel(&self) {
            let mut c = self.children.lock().unwrap_or_else(|e| e.into_inner());
            c.cancelled = true;
            for child in &c.running {
                // A child being reaped holds its lock: it has already
                // closed its output and is exiting.
                if let Ok(mut ch) = child.try_lock() {
                    let _ = ch.kill();
                }
            }
        }

        /// Starts `git -C <cwd> <args>`; `unavailable` when it cannot.
        fn spawn(
            &self,
            args: &[&str],
        ) -> Result<(Arc<Mutex<Child>>, std::process::ChildStdout), Outcome> {
            let mut cmd = Command::new(&self.git);
            cmd.arg("-C")
                .arg(&self.cwd)
                .args(args)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            for v in KEPT_VARS {
                if let Some(value) = std::env::var_os(v) {
                    cmd.env(v, value);
                }
            }
            cmd.env("GIT_OPTIONAL_LOCKS", "0")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("LC_ALL", "C")
                .env("GIT_PAGER", "cat")
                // Never fetch a partial clone's missing objects (git 2.44+).
                .env("GIT_NO_LAZY_FETCH", "1")
                .env("GIT_CONFIG_COUNT", self.config.len().to_string());
            for (i, (k, v)) in self.config.iter().enumerate() {
                cmd.env(format!("GIT_CONFIG_KEY_{i}"), k)
                    .env(format!("GIT_CONFIG_VALUE_{i}"), v);
            }
            let mut c = self.children.lock().unwrap_or_else(|e| e.into_inner());
            if c.cancelled {
                return Err("timeout");
            }
            let mut child = cmd.spawn().map_err(|_| "unavailable")?;
            let stdout = child.stdout.take().ok_or("unavailable")?;
            let child = Arc::new(Mutex::new(child));
            c.running.push(child.clone());
            Ok((child, stdout))
        }

        /// Reaps `child` once its output has ended; true when it exited 0.
        fn reap(child: &Mutex<Child>) -> bool {
            let mut ch = child.lock().unwrap_or_else(|e| e.into_inner());
            ch.wait().is_ok_and(|s| s.success())
        }

        /// Runs a command to completion, keeping its standard output.
        fn output(&self, args: &[&str]) -> Result<Out, Outcome> {
            let (child, mut stdout) = self.spawn(args)?;
            let mut buf = Vec::new();
            let read = stdout.read_to_end(&mut buf).is_ok();
            drop(stdout);
            let ok = Self::reap(&child) && read;
            Ok(Out { ok, stdout: buf })
        }

        /// Runs a diff, hashing its output as it streams, up to [`DIFF_CAP`].
        fn diff(&self, args: &[&str]) -> Result<Diff, Outcome> {
            let (child, mut stdout) = self.spawn(args)?;
            let mut hash = Sha256::new();
            let mut bytes = 0u64;
            let mut truncated = false;
            let mut buf = vec![0u8; 64 << 10];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let take = (n as u64).min(DIFF_CAP - bytes) as usize;
                        hash.update(&buf[..take]);
                        bytes += take as u64;
                        if take < n || bytes == DIFF_CAP {
                            truncated = take < n || stdout.read(&mut buf).is_ok_and(|m| m > 0);
                            if truncated {
                                break;
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return Err("unavailable"),
                }
            }
            drop(stdout);
            if truncated {
                let _ = child.lock().unwrap_or_else(|e| e.into_inner()).kill();
                Self::reap(&child);
            } else if !Self::reap(&child) {
                return Err("unavailable");
            }
            Ok(Diff {
                sha256: format!("sha256:{}", hex(&hash.finalize())),
                bytes,
                truncated,
            })
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Runs `job` on its own thread, its result sent on `tx`.
    fn job<T: Send + 'static>(
        tx: &mpsc::Sender<Part>,
        wrap: fn(Result<T, Outcome>) -> Part,
        job: impl FnOnce() -> Result<T, Outcome> + Send + 'static,
    ) {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(wrap(job()));
        });
    }

    /// The branch, and the remote with its sanitized URL.
    type Remote = (Option<String>, Option<(String, String)>);
    /// HEAD, the diff of the tree against it, and whether the diff was
    /// unavailable.
    type Head = (Option<String>, Option<Diff>, bool);
    /// Whether the tree is dirty, and its count of untracked paths.
    type Status = (bool, u64);

    enum Part {
        Root(Result<String, Outcome>),
        Filters(Result<Scan, Outcome>),
        Remote(Result<Remote, Outcome>),
        Head(Result<Head, Outcome>),
        Status(Result<Status, Outcome>),
    }

    /// Waits for the next part until `deadline`.
    fn next(rx: &mpsc::Receiver<Part>, deadline: Instant) -> Result<Part, Outcome> {
        let left = deadline.saturating_duration_since(Instant::now());
        rx.recv_timeout(left).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => "timeout",
            mpsc::RecvTimeoutError::Disconnected => "unavailable",
        })
    }

    pub(super) fn run(r: &Runner, deadline: Instant) -> Result<GitContext, Outcome> {
        let (tx, rx) = mpsc::channel();
        let root = r.clone();
        job(&tx, Part::Root, move || {
            root.output(&["rev-parse", "--show-toplevel"])?
                .line()?
                .ok_or("not-a-repo")
        });
        let rf = r.clone();
        job(&tx, Part::Filters, move || filters(&rf));
        let (mut repo_root, mut scan) = (None, None);
        while repo_root.is_none() || scan.is_none() {
            match next(&rx, deadline)? {
                Part::Root(v) => repo_root = Some(v?),
                Part::Filters(v) => scan = Some(v?),
                _ => return Err("unavailable"),
            }
        }
        let repo_root = repo_root.unwrap_or_default();
        let scan = scan.unwrap_or_default();
        let r = &r.without_filters(&scan.drivers);

        let rr = r.clone();
        job(&tx, Part::Remote, move || remote(&rr));
        let rh = r.clone();
        let partial = scan.partial_clone;
        job(&tx, Part::Head, move || head(&rh, partial));
        let rs = r.clone();
        job(&tx, Part::Status, move || status(&rs));
        drop(tx);
        let (mut rem, mut hd, mut st) = (None, None, None);
        while rem.is_none() || hd.is_none() || st.is_none() {
            match next(&rx, deadline)? {
                Part::Remote(v) => rem = Some(v?),
                Part::Head(v) => hd = Some(v?),
                Part::Status(v) => st = Some(v?),
                Part::Root(_) | Part::Filters(_) => return Err("unavailable"),
            }
        }
        let ((branch, remote), (head, diff, diff_unavailable), (dirty, untracked)) = (
            rem.unwrap_or_default(),
            hd.unwrap_or_default(),
            st.unwrap_or_default(),
        );
        let diff = diff.filter(|d| d.bytes > 0);
        let dirty = dirty || diff.is_some();
        let (remote, remote_url) = remote.unzip();
        Ok(GitContext {
            repo_root,
            remote,
            remote_url,
            branch,
            head,
            dirty,
            diff_sha256: diff.as_ref().map(|d| d.sha256.clone()),
            diff_bytes: diff.as_ref().map(|d| d.bytes),
            diff_truncated: diff.as_ref().is_some_and(|d| d.truncated),
            diff_unavailable: diff_unavailable && dirty,
            untracked: if dirty { untracked } else { 0 },
            captured_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        })
    }

    /// What the configuration says before anything else runs.
    #[derive(Default)]
    struct Scan {
        /// The filter drivers it names (`filter.<driver>.*`).
        drivers: Vec<String>,
        /// Whether the repository is a partial clone
        /// (`extensions.partialClone`, or a remote marked `promisor`).
        partial_clone: bool,
    }

    /// Reads the [`Scan`] with `git config`, which runs nothing. A
    /// configuration that cannot be read names nothing; the commands that
    /// follow then fail on it too.
    fn filters(r: &Runner) -> Result<Scan, Outcome> {
        let out = r.output(&[
            "config",
            "-z",
            "--get-regexp",
            "^(filter\\..*|extensions\\.partialclone|remote\\..*\\.promisor)$",
        ])?;
        let mut scan = Scan::default();
        for entry in out.stdout.split(|b| *b == 0).filter(|k| !k.is_empty()) {
            // The key must be UTF-8; a value (a filter's command, say) is
            // compared as bytes and may be anything.
            let (key, value) = match entry.iter().position(|b| *b == b'\n') {
                Some(i) => (&entry[..i], &entry[i + 1..]),
                None => (entry, &[][..]),
            };
            let key = std::str::from_utf8(key).map_err(|_| "unavailable")?;
            if let Some(rest) = key.strip_prefix("filter.") {
                let Some((driver, _)) = rest.rsplit_once('.') else {
                    continue;
                };
                if !scan.drivers.iter().any(|d| d == driver) {
                    scan.drivers.push(driver.to_string());
                }
            } else if key == "extensions.partialclone" {
                scan.partial_clone |= !value.is_empty();
            } else if key.ends_with(".promisor") {
                scan.partial_clone |= !matches!(
                    value.to_ascii_lowercase().as_slice(),
                    b"false" | b"no" | b"off" | b"0"
                );
            }
        }
        if scan.drivers.len() > MAX_FILTERS {
            return Err("unavailable");
        }
        Ok(scan)
    }

    /// The first line of `git version`, run by `r` and waited for until
    /// `deadline`; `None` when git cannot be run or does not answer.
    pub(super) fn version(r: &Runner, deadline: Instant) -> Option<String> {
        let (tx, rx) = mpsc::channel();
        let rv = r.clone();
        job(&tx, Part::Root, move || {
            rv.output(&["version"])?.line()?.ok_or("unavailable")
        });
        match next(&rx, deadline) {
            Ok(Part::Root(Ok(line))) => Some(line),
            _ => None,
        }
    }

    /// The branch, then its upstream remote (else `origin`, else the first
    /// remote) and that remote's URL, sanitized.
    fn remote(r: &Runner) -> Result<Remote, Outcome> {
        let branch = r
            .output(&["symbolic-ref", "-q", "--short", "HEAD"])?
            .line()?;
        let upstream = match &branch {
            Some(b) => r
                .output(&["config", "--get", &format!("branch.{b}.remote")])?
                .line()?
                .filter(|v| v != "."),
            None => None,
        };
        let name = match upstream {
            Some(u) => Some(u),
            None => {
                let list = r.output(&["remote"])?;
                let text = String::from_utf8(list.stdout).map_err(|_| "invalid")?;
                let names: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
                names
                    .iter()
                    .find(|n| **n == "origin")
                    .or(names.first())
                    .map(|n| n.to_string())
            }
        };
        let remote = match name {
            Some(n) => r
                .output(&["remote", "get-url", &n])?
                .line()?
                .map(|url| (n, sanitize_remote(&url))),
            None => None,
        };
        Ok((branch, remote))
    }

    /// HEAD, then the diff of staged and unstaged changes to tracked files
    /// against it (against the empty tree on an unborn branch). In a
    /// partial clone the diff may need objects that are not there; git
    /// refuses to fetch them (`GIT_NO_LAZY_FETCH`), so such a diff fails and
    /// is unavailable.
    fn head(r: &Runner, partial_clone: bool) -> Result<Head, Outcome> {
        let head = r.output(&["rev-parse", "-q", "--verify", "HEAD"])?.line()?;
        let mut args = vec!["diff"];
        args.push(if head.is_some() { "HEAD" } else { "--cached" });
        args.extend(super::DIFF_FLAGS);
        match r.diff(&args) {
            Ok(diff) => Ok((head, Some(diff), false)),
            Err("unavailable") if partial_clone => Ok((head, None, true)),
            Err(e) => Err(e),
        }
    }

    /// Whether `git status` prints anything, and how many untracked paths
    /// it names.
    fn status(r: &Runner) -> Result<Status, Outcome> {
        let out = r.output(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=dirty",
            "--no-renames",
        ])?;
        if !out.ok {
            return Err("unavailable");
        }
        let mut untracked = 0;
        // `--no-renames`: every entry is one field, never a rename's two.
        for f in out.stdout.split(|b| *b == 0).filter(|f| !f.is_empty()) {
            if f.starts_with(b"??") {
                untracked += 1;
            }
        }
        Ok((!out.stdout.is_empty(), untracked))
    }
}

#[cfg(test)]
#[path = "gitctx_capture_tests.rs"]
mod capture_tests;

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
            diff_unavailable: false,
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

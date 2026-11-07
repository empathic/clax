//! The host's own clone: the only copy with a write credential, kept away
//! from the sandbox. A patch from a sandbox is applied here and judged here,
//! from what this git says it applied.
//!
//! The clone has no working tree: a patch goes into the index and from there
//! into a commit, so nothing a patch names is ever written as a file on the
//! host, and a small patch cannot multiply a big file across its disk.
//!
//! Every git runs with an environment of [`GIT_ENV`] and no other, no user or
//! system configuration, no template and no hooks, under [`GIT_TIMEOUT`]. A
//! token comes from [`RepoAccess`] for one command at a time and travels in
//! that command's environment rather than in `argv` or a config file. What
//! git writes can still quote a token, as when it shows a remote's own error
//! page, so its text passes through [`redact`] before it goes anywhere.
//!
//! A clone made with a [`PublicOnly`] rule holds each command that reaches the
//! remote to it, as [`crate::net`] says.

use crate::forge::TreeChange;
use crate::net::PublicOnly;
use anyhow::{Context, Result, bail};
use base64::Engine;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{fmt, future::Future, pin::Pin};
use tokio::io::AsyncWriteExt;

/// The address ClaxBot commits as.
const BOT_EMAIL: &str = "claxbot@users.noreply.clax.dev";

/// The most of git's error text that is kept, after redaction.
const MAX_GIT_TEXT: usize = 2000;

/// The most any one git command may take: a big clone is the longest, and
/// gets as long as a container's.
const GIT_TIMEOUT: Duration = Duration::from_secs(600);

/// What a host's git keeps of the daemon's environment: where it is, and how
/// the host reaches the network.
const GIT_ENV: [&str; 14] = [
    "PATH",
    "TMPDIR",
    "DEVELOPER_DIR",
    "http_proxy",
    "https_proxy",
    "HTTPS_PROXY",
    "no_proxy",
    "NO_PROXY",
    "all_proxy",
    "ALL_PROXY",
    "GIT_SSL_CAINFO",
    "CURL_CA_BUNDLE",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// How many issued tokens a [`Redactor`] remembers.
const REMEMBERED_TOKENS: usize = 256;

/// A credential. Its `Debug` shows nothing of it.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn new(secret: impl Into<String>) -> Token {
        Token(secret.into())
    }

    /// The secret, for the one call that sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token([redacted])")
    }
}

/// Where a target's credentials come from. It is asked for each clone, push
/// and forge call and what it answers is not kept past that, so a token may
/// be short-lived; caching within its lifetime is the implementation's.
#[async_trait::async_trait]
pub trait RepoAccess: Send + Sync {
    /// For a container's one clone; `None` for a public repository. Never
    /// one that can write.
    async fn read_token(&self) -> Result<Option<Token>>;

    /// For the host's clone, its pushes and the forge; `None` where the
    /// remote asks for none.
    async fn write_token(&self) -> Result<Option<Token>>;

    /// Ends a read token once its clone is over, where tokens can be ended.
    async fn revoke_read(&self, _token: &Token) -> Result<()> {
        Ok(())
    }

    /// The forge refused `token`: it is not to be handed out again.
    async fn write_refused(&self, _token: &Token) {}
}

/// Tokens fixed for the life of the daemon.
#[derive(Clone, Debug, Default)]
pub struct StaticAccess {
    pub read: Option<Token>,
    pub write: Option<Token>,
}

#[async_trait::async_trait]
impl RepoAccess for StaticAccess {
    async fn read_token(&self) -> Result<Option<Token>> {
        Ok(self.read.clone())
    }

    async fn write_token(&self) -> Result<Option<Token>> {
        Ok(self.write.clone())
    }
}

/// Remembers the tokens issued, to take them out of text. A token that has
/// expired can still be quoted, and is still worth keeping out of a log.
#[derive(Debug, Default)]
pub struct Redactor {
    issued: Mutex<VecDeque<Token>>,
}

impl Redactor {
    pub fn note(&self, token: &Token) {
        let mut issued = self.issued.lock().expect("the redactor's lock");
        if !issued.contains(token) {
            issued.push_back(token.clone());
        }
        if issued.len() > REMEMBERED_TOKENS {
            issued.pop_front();
        }
    }

    /// `text` with every token noted gone.
    pub fn clean(&self, text: &str) -> String {
        let issued = self.issued.lock().expect("the redactor's lock");
        redact(text, issued.iter().map(Token::expose))
    }
}

/// `inner`, noting all it issues in a [`Redactor`]. For a holder of tokens
/// outside a workspace, such as its forge.
pub struct Remembered {
    pub inner: Arc<dyn RepoAccess>,
    pub redactor: Arc<Redactor>,
}

impl Remembered {
    fn noted(&self, token: Option<Token>) -> Option<Token> {
        if let Some(t) = &token {
            self.redactor.note(t);
        }
        token
    }
}

#[async_trait::async_trait]
impl RepoAccess for Remembered {
    async fn read_token(&self) -> Result<Option<Token>> {
        Ok(self.noted(self.inner.read_token().await?))
    }

    async fn write_token(&self) -> Result<Option<Token>> {
        Ok(self.noted(self.inner.write_token().await?))
    }

    async fn revoke_read(&self, token: &Token) -> Result<()> {
        self.inner.revoke_read(token).await
    }

    async fn write_refused(&self, token: &Token) {
        self.inner.write_refused(token).await;
    }
}

/// What holds a target's runs: what refuses a patch, how much a target takes,
/// and what a reporter's word may start.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Paths a patch may not touch, whatever their case. Each is a
    /// [`path pattern`](check_pattern).
    pub protected_paths: Vec<String>,
    /// Where a patch may write, as [`path patterns`](check_pattern) in the
    /// repository's own case; empty sets no limit. `protected_paths` subtract
    /// from it, so the two together stay right if this grows.
    pub writable_paths: Vec<String>,
    /// Text a change may not add to a `.rs` file, whitespace aside: `#[path`
    /// can point a `mod` at a protected file from a writable one.
    pub refused_in_rust: Vec<String>,
    /// The most files a patch may touch, which also bounds the entries it
    /// can add to the index.
    pub max_files: usize,
    /// The most a patch's binary files may inflate to, together.
    pub max_binary_bytes: u64,
    /// The most the files of a commit made by a forge may hold, together:
    /// each is uploaded whole, however small the edit to it.
    pub max_change_bytes: u64,
    /// The most open pull requests ClaxBot's branches may hold at once: a
    /// change that would open another is refused before it is pushed.
    pub max_open_pull_requests: usize,
    /// The most pushes the target takes in any 24 hours.
    pub max_pushes_per_day: usize,
    /// That a reporter's word on a fix may start a run with no person
    /// sending it ([`clax_bot::workspace::Offer::no_follow_ups`]).
    pub follow_ups: bool,
    /// The address of a report, with `{report}` for its ID, for a pull
    /// request to link back to. Without it the ID stands alone.
    pub report_url: Option<String>,
    /// [`clax_bot::workspace::Offer::guidance`].
    pub guidance: Option<String>,
    /// [`clax_bot::workspace::Offer::preview_gate`].
    pub preview_gate: Option<String>,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            protected_paths: vec![
                ".github/".into(),
                ".gitmodules".into(),
                "**/.gitattributes".into(),
                "**/.lfsconfig".into(),
            ],
            writable_paths: Vec::new(),
            refused_in_rust: Vec::new(),
            max_files: 50,
            max_binary_bytes: 32 * 1024 * 1024,
            max_change_bytes: 32 * 1024 * 1024,
            max_open_pull_requests: 50,
            max_pushes_per_day: 200,
            follow_ups: true,
            report_url: None,
            guidance: None,
            preview_gate: None,
        }
    }
}

impl Policy {
    /// Whether a patch may not touch `path`, in any case.
    pub fn protects(&self, path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        self.protected_paths
            .iter()
            .any(|p| pattern_matches(&p.to_ascii_lowercase(), &lower))
    }

    /// Whether `path` is inside [`Self::writable_paths`], in the repository's
    /// own case: a checkout that folds case must not widen what is allowed.
    pub fn may_write(&self, path: &str) -> bool {
        self.writable_paths.is_empty()
            || self.writable_paths.iter().any(|p| pattern_matches(p, path))
    }

    /// Whether a person reviewing a change should be told it touches `path`:
    /// this target refuses it, or it runs when the repository is built,
    /// whatever this target lists. A branch holds what no run was judged on,
    /// so a target's refusals alone would not say.
    pub fn sensitive(&self, path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        self.protects(path)
            || !self.may_write(path)
            || BUILD_TIME_PATHS
                .iter()
                .any(|p| pattern_matches(&p.to_ascii_lowercase(), &lower))
    }
}

/// What most repositories run when they are built, for [`Policy::sensitive`].
const BUILD_TIME_PATHS: [&str; 17] = [
    ".github/",
    "**/Cargo.toml",
    "**/Cargo.lock",
    "**/build.rs",
    "**/package.json",
    "**/package-lock.json",
    "**/pnpm-lock.yaml",
    "**/yarn.lock",
    "**/.npmrc",
    "**/BUILD.bazel",
    "**/BUILD",
    "**/MODULE.bazel",
    "**/WORKSPACE",
    "**/Dockerfile",
    "**/Makefile",
    "**/justfile",
    "**/rust-toolchain.toml",
];

/// Whether one path segment of a pattern, where each `*` stands for any run
/// of characters, is `text`.
fn segment_matches(pattern: &str, text: &str) -> bool {
    let Some((head, rest)) = pattern.split_once('*') else {
        return pattern == text;
    };
    let Some(tail) = text.strip_prefix(head) else {
        return false;
    };
    tail.char_indices()
        .map(|(i, _)| i)
        .chain([tail.len()])
        .any(|i| segment_matches(rest, &tail[i..]))
}

/// Whether [`path pattern`](check_pattern) `pattern` names `path`.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    if let Some(name) = pattern.strip_prefix("**/") {
        return path
            .rsplit('/')
            .next()
            .is_some_and(|last| segment_matches(name, last));
    }
    let under = pattern.ends_with('/');
    let wanted: Vec<&str> = pattern.trim_end_matches('/').split('/').collect();
    let got: Vec<&str> = path.split('/').collect();
    let fits = if under {
        got.len() >= wanted.len()
    } else {
        got.len() == wanted.len()
    };
    fits && wanted.iter().zip(&got).all(|(w, g)| segment_matches(w, g))
}

/// Why `pattern` is no path pattern. There are three forms, and a `*` stands
/// for any run of characters inside one path segment: `a/*/c.rs`, one path;
/// `a/*/c/`, everything under such a directory; `**/name`, a file of that
/// name at any depth, the root too.
pub fn check_pattern(pattern: &str) -> Result<(), String> {
    let rest = pattern.strip_prefix("**/");
    let anywhere = rest.is_some();
    let rest = rest.unwrap_or(pattern);
    let segments: Vec<&str> = rest.strip_suffix('/').unwrap_or(rest).split('/').collect();
    let bad = |s: &&str| s.is_empty() || matches!(*s, "." | "..") || s.contains("**");
    if segments.iter().any(bad) {
        return Err(format!("{pattern:?} is no path pattern"));
    }
    if anywhere && (segments.len() > 1 || rest.ends_with('/')) {
        return Err(format!("{pattern:?}: `**/` takes one file's name"));
    }
    Ok(())
}

/// The remote refused the credentials, and asking again will not change
/// that. The text is for the log.
#[derive(Debug)]
pub struct AccessRefused(pub String);

impl fmt::Display for AccessRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AccessRefused {}

/// The remote would not take a push, which the same push will not change:
/// the branch moved, or a rule of its own stopped it. The text is for the log.
#[derive(Debug)]
pub struct PushRefused(pub String);

impl fmt::Display for PushRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PushRefused {}

/// Whether `text`, what a failed `git` said, is a remote refusing its
/// credentials. A forge answers a private repository it will not show with
/// a 401, a 403 or as though it were not there.
pub fn refuses_access(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    let says = |s: &str| text.contains(s);
    says("authentication failed")
        || says("could not read username")
        || says("could not read password")
        || says("invalid username or")
        || says("returned error: 401")
        || says("returned error: 403")
        || says("repository not found")
        || (says("repository '") && says("not found"))
}

fn basic_credentials(token: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"))
}

/// The environment that passes `token` to git as an HTTP header, for
/// one command. The header goes to every request of that command, so a
/// redirect is refused rather than followed to wherever it leads.
pub fn token_env(token: &Token) -> BTreeMap<String, String> {
    let basic = basic_credentials(token.expose());
    BTreeMap::from([
        ("GIT_CONFIG_COUNT".to_string(), "2".to_string()),
        (
            "GIT_CONFIG_KEY_0".to_string(),
            "http.extraHeader".to_string(),
        ),
        (
            "GIT_CONFIG_VALUE_0".to_string(),
            format!("Authorization: Basic {basic}"),
        ),
        (
            "GIT_CONFIG_KEY_1".to_string(),
            "http.followRedirects".to_string(),
        ),
        ("GIT_CONFIG_VALUE_1".to_string(), "false".to_string()),
    ])
}

/// `text` with each of `tokens` gone, as itself and as the credentials
/// [`token_env`] makes of it.
pub fn redact<'a>(text: &str, tokens: impl IntoIterator<Item = &'a str>) -> String {
    let mut text = text.to_string();
    for token in tokens.into_iter().filter(|t| !t.is_empty()) {
        text = text
            .replace(&basic_credentials(token), "[redacted]")
            .replace(token, "[redacted]");
    }
    text
}

fn clip(text: &str) -> String {
    text.chars().take(MAX_GIT_TEXT).collect()
}

/// A problem with `patch`, read before git sees it. A `literal` hunk declares
/// what it inflates to, and git allocates that; a `delta` can expand without
/// limit, and only inflating it shows by how much. Neither is a line of a
/// text hunk, whose lines all start with a marker. A `diff --git` line is a
/// file, each an entry in the index however little of the patch it took.
fn patch_refusal(patch: &[u8], policy: &Policy) -> Option<String> {
    let mut total: u64 = 0;
    let mut files = 0;
    for line in patch.split(|b| *b == b'\n') {
        if line.starts_with(b"diff --git ") {
            files += 1;
        }
        if let Some(why) = mode_refusal(line) {
            return Some(why);
        }
        if line.starts_with(b"delta ") {
            return Some("a change to an existing binary file is refused".to_string());
        }
        let Some(size) = line.strip_prefix(b"literal ") else {
            continue;
        };
        let Some(size) = declared_size(size) else {
            return Some("a binary hunk does not say its size".to_string());
        };
        total = total.saturating_add(size);
    }
    if files > policy.max_files {
        return Some(format!("{files} files is over the limit"));
    }
    (total > policy.max_binary_bytes)
        .then(|| format!("{total} bytes of binary files is over the limit"))
}

/// A problem with the mode a patch's header `line` declares, if it declares
/// one: `git apply` takes what the header says, so only a plain file passes.
/// A symlink or a submodule goes on to [`refusal`], which says what it is.
fn mode_refusal(line: &[u8]) -> Option<String> {
    let mode = [
        &b"new file mode "[..],
        b"deleted file mode ",
        b"old mode ",
        b"new mode ",
    ]
    .iter()
    .find_map(|p| line.strip_prefix(*p))
    .or_else(|| {
        let index = line.strip_prefix(b"index ")?;
        let mut words = index.split(u8::is_ascii_whitespace);
        words.find(|w| !w.is_empty())?;
        words.find(|w| !w.is_empty())
    })?
    .trim_ascii();
    (!matches!(mode, b"100644" | b"100755" | b"120000" | b"160000")).then(|| {
        let mode = String::from_utf8_lossy(mode);
        format!("mode {mode} is not a plain file's")
    })
}

/// The number `strtoul` would read from `text`, as git does; `None` for
/// anything it would read as other than a plain number.
fn declared_size(text: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(text).ok()?.trim();
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().unwrap_or(u64::MAX))
}

/// Whether `new` holds more of `pattern`, whitespace aside, than `old`.
fn adds_text(old: &[u8], new: &[u8], pattern: &str) -> bool {
    let count = |bytes: &[u8]| {
        let text = String::from_utf8_lossy(bytes);
        let squeezed: String = text.split_whitespace().collect();
        squeezed.matches(pattern).count()
    };
    count(new) > count(old)
}

/// One entry of `git diff --raw -z`, for each path it names.
struct Touched<'a> {
    old_mode: &'a str,
    new_mode: &'a str,
    old_id: &'a str,
    new_id: &'a str,
    path: &'a str,
}

/// The entries of `text`, or what is wrong with it.
fn touched(text: &str) -> Result<Vec<Touched<'_>>, String> {
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    let mut all = Vec::new();
    while let Some(meta) = fields.next() {
        // `:old new oldsha newsha status`, then one path, or two for a
        // rename or copy.
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let [old_mode, new_mode, old_id, new_id, status] = parts[..] else {
            return Err(format!("unreadable diff entry: {meta}"));
        };
        let paths = if status.starts_with(['R', 'C']) { 2 } else { 1 };
        for _ in 0..paths {
            let path = fields.next().ok_or("a diff entry names no path")?;
            all.push(Touched {
                old_mode,
                new_mode,
                old_id,
                new_id,
                path,
            });
        }
    }
    Ok(all)
}

/// A problem with what a patch touches, read from `git diff --raw -z`.
pub fn refusal(raw: &[u8], policy: &Policy) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let all = match touched(&text) {
        Ok(all) => all,
        Err(why) => return Some(why),
    };
    for t in &all {
        let path = t.path;
        for mode in [t.old_mode, t.new_mode] {
            if mode == "120000" || mode == "160000" {
                return Some(format!("{path} is a symlink or a submodule"));
            }
        }
        let absent = "000000";
        if t.old_mode != t.new_mode && t.old_mode != absent && t.new_mode != absent {
            let (old, new) = (t.old_mode, t.new_mode);
            return Some(format!("{path} changes mode: {old} to {new}"));
        }
        if policy.protects(path) {
            return Some(format!("{path} is protected"));
        }
        if !policy.may_write(path) {
            return Some(format!("{path} is outside what this repository takes"));
        }
    }
    let files = all.len();
    (files > policy.max_files).then(|| format!("{files} files is over the limit"))
}

/// A path of `raw`'s that differs only by case from another in `tree`, the
/// NUL-separated paths of the whole index: on a case-insensitive checkout
/// one would overwrite the other.
fn case_collision(raw: &[u8], tree: &[u8]) -> Option<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for path in tree.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let lower = String::from_utf8_lossy(path).to_lowercase();
        *seen.entry(lower).or_default() += 1;
    }
    let text = String::from_utf8_lossy(raw);
    let all = touched(&text).ok()?;
    let hit = all
        .iter()
        .find(|t| seen.get(&t.path.to_lowercase()).is_some_and(|n| *n > 1))?;
    Some(format!(
        "{} differs from another path only by case",
        hit.path
    ))
}

/// The clone.
pub struct Clone {
    dir: PathBuf,
    /// Where `origin` is.
    url: String,
    /// What holds each command that reaches `url`, when anything does.
    public_only: Option<PublicOnly>,
}

impl fmt::Debug for Clone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Clone").field("dir", &self.dir).finish()
    }
}

/// One git command's result.
struct Run {
    ok: bool,
    stdout: Vec<u8>,
    /// Redacted, then clipped.
    stderr: String,
}

/// The future of one git command.
type Running<'a> = Pin<Box<dyn Future<Output = Result<std::process::Output>> + Send + 'a>>;

impl Clone {
    /// Clones `url` into `dir`, which must not exist, reading with `token`.
    pub async fn new(url: &str, dir: &Path, token: Option<&Token>) -> Result<Clone> {
        Clone::reaching(url, dir, token, None).await
    }

    /// [`Clone::new`], every command that reaches `url` held to
    /// `public_only`, when it is given.
    pub async fn reaching(
        url: &str,
        dir: &Path,
        token: Option<&Token>,
        public_only: Option<PublicOnly>,
    ) -> Result<Clone> {
        let parent = Clone {
            dir: dir.parent().context("the clone's parent")?.to_path_buf(),
            url: url.to_string(),
            public_only: public_only.clone(),
        };
        let path = dir.to_string_lossy();
        let args = [
            "clone",
            "--quiet",
            "--no-hardlinks",
            "--no-checkout",
            "--template=",
            "--",
            url,
            &path,
        ];
        parent.reach(&args, token).await?;
        Ok(Clone {
            dir: dir.to_path_buf(),
            url: url.to_string(),
            public_only,
        })
    }

    /// [`Self::must`], for a command that reaches the remote.
    async fn reach(&self, args: &[&str], token: Option<&Token>) -> Result<Vec<u8>> {
        let pin = match &self.public_only {
            Some(rule) => rule.pin(&self.url).await?,
            None => Vec::new(),
        };
        let mut all: Vec<&str> = pin.iter().map(String::as_str).collect();
        all.extend_from_slice(args);
        let r = self.git(&all, None, token).await?;
        if r.ok {
            return Ok(r.stdout);
        }
        let said = format!("git {}: {}", args[0], r.stderr);
        if refuses_access(&r.stderr) {
            return Err(AccessRefused(said).into());
        }
        if r.stderr.contains("[rejected]") || r.stderr.contains("[remote rejected]") {
            return Err(PushRefused(said).into());
        }
        bail!("{said}")
    }

    fn start(&self, args: &[&str], stdin: Option<&[u8]>, token: Option<&Token>) -> Running<'_> {
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .current_dir(&self.dir)
            .env_clear()
            .envs(
                GIT_ENV
                    .iter()
                    .filter_map(|k| Some((k, std::env::var_os(k)?))),
            )
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(t) = token {
            cmd.envs(token_env(t));
        }
        let input = stdin.map(<[u8]>::to_vec);
        Box::pin(async move {
            let mut child = cmd.spawn().context("starting git")?;
            if let Some(input) = input {
                let mut pipe = child.stdin.take().context("git's stdin")?;
                // A git that stopped reading says why when it exits.
                if pipe.write_all(&input).await.is_ok() {
                    let _ = pipe.shutdown().await;
                }
            }
            child.wait_with_output().await.context("waiting for git")
        })
    }

    async fn git(&self, args: &[&str], stdin: Option<&[u8]>, token: Option<&Token>) -> Result<Run> {
        let out = tokio::time::timeout(GIT_TIMEOUT, self.start(args, stdin, token))
            .await
            .with_context(|| format!("git {} did not finish", args[0]))??;
        let stderr = String::from_utf8_lossy(&out.stderr);
        let secret = token.map(Token::expose);
        Ok(Run {
            ok: out.status.success(),
            stdout: out.stdout,
            stderr: clip(&redact(stderr.trim(), secret)),
        })
    }

    async fn must(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        token: Option<&Token>,
    ) -> Result<Vec<u8>> {
        let r = self.git(args, stdin, token).await?;
        if !r.ok {
            bail!("git {}: {}", args[0], r.stderr);
        }
        Ok(r.stdout)
    }

    /// Stands on `branch` at `from`, the index with it.
    async fn stand_on(&self, branch: &str, from: &str) -> Result<()> {
        let local = format!("refs/heads/{branch}");
        self.must(&["update-ref", &local, from], None, None).await?;
        self.must(&["symbolic-ref", "HEAD", &local], None, None)
            .await?;
        self.must(&["read-tree", "HEAD"], None, None).await?;
        Ok(())
    }

    /// Stands on `branch`: as on the remote when it is there, else new
    /// from `base`. Answers the commit it stands on.
    pub async fn start_branch(&self, branch: &str, base: &str) -> Result<String> {
        let from = if self.on_remote(branch).await? {
            format!("refs/remotes/origin/{branch}")
        } else {
            format!("refs/remotes/origin/{base}")
        };
        self.stand_on(branch, &from).await?;
        self.head().await
    }

    /// The branches on the remote, as of the clone, that are `name` or
    /// `name-N`, each with its number (`name` is 1), the highest first. A
    /// number that no next one can follow is no branch of ours.
    pub async fn numbered_branches(&self, name: &str) -> Result<Vec<(u32, String)>> {
        let refs = [
            "for-each-ref",
            "--format=%(refname)",
            "refs/remotes/origin/",
        ];
        let out = self.must(&refs, None, None).await?;
        let text = String::from_utf8_lossy(&out);
        let mut found: Vec<(u32, String)> = text
            .lines()
            .filter_map(|r| r.strip_prefix("refs/remotes/origin/"))
            .filter_map(|b| {
                let rest = b.strip_prefix(name)?;
                let n = match rest.strip_prefix('-') {
                    Some(n) => n.parse().ok().filter(|n| *n < u32::MAX)?,
                    None if rest.is_empty() => 1,
                    None => return None,
                };
                Some((n, b.to_string()))
            })
            .collect();
        found.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        Ok(found)
    }

    async fn on_remote(&self, branch: &str) -> Result<bool> {
        Ok(self.remote_tip(branch).await?.is_some())
    }

    /// Stands on `branch` as a run that started from commit `base` left it.
    /// Its tip on the remote must be `base` or one of `pushed`, the commits
    /// the run is known to have pushed; answers why not, since a commit of
    /// anyone else's there would be undone by the run's next proposal, which
    /// is the whole of its change on top of `base`.
    pub async fn resume_branch(
        &self,
        branch: &str,
        base: &str,
        pushed: &[&str],
    ) -> Result<Option<String>> {
        let commit = format!("{base}^{{commit}}");
        let known = self
            .git(&["rev-parse", "--verify", "--quiet", &commit], None, None)
            .await?;
        if !known.ok {
            return Ok(Some(format!("commit {base} is not in the repository")));
        }
        if !self.on_remote(branch).await? {
            self.stand_on(branch, base).await?;
            return Ok(None);
        }
        self.stand_on(branch, &format!("refs/remotes/origin/{branch}"))
            .await?;
        let tip = self.head().await?;
        if tip != base && !pushed.contains(&tip.as_str()) {
            return Ok(Some(format!("{branch} moved to {tip} under the run")));
        }
        Ok(None)
    }

    /// The commit `branch` stood on at the remote when last heard from: at
    /// the clone, or at a push that succeeded here.
    pub async fn remote_tip(&self, branch: &str) -> Result<Option<String>> {
        let remote = format!("refs/remotes/origin/{branch}");
        let r = self
            .git(&["rev-parse", "--verify", "--quiet", &remote], None, None)
            .await?;
        Ok(r.ok.then(|| String::from_utf8_lossy(&r.stdout).trim().to_string()))
    }

    pub async fn head(&self) -> Result<String> {
        let out = self.must(&["rev-parse", "HEAD"], None, None).await?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// Applies `patch`, made against commit `base`, to the index; a refusal
    /// comes back as `Ok(Some(why))`, and the index stays at `HEAD`. `base`
    /// is an ancestor of `HEAD` or `HEAD` itself: the index ends as `base`
    /// plus `patch`, which a run's container, knowing nothing of what was
    /// committed here, holds as the whole of its change. It is judged as
    /// that whole.
    pub async fn apply_onto(
        &self,
        base: &str,
        patch: &[u8],
        policy: &Policy,
    ) -> Result<Option<String>> {
        if let Some(why) = patch_refusal(patch, policy) {
            return Ok(Some(why));
        }
        self.must(&["read-tree", base], None, None).await?;
        let why = self.judge(base, patch, policy).await?;
        if why.is_some() {
            self.must(&["read-tree", "HEAD"], None, None).await?;
        }
        Ok(why)
    }

    async fn judge(&self, base: &str, patch: &[u8], policy: &Policy) -> Result<Option<String>> {
        let r = self
            .git(
                &["apply", "--cached", "--whitespace=nowarn", "-"],
                Some(patch),
                None,
            )
            .await?;
        if !r.ok {
            return Ok(Some(format!("the patch does not apply: {}", r.stderr)));
        }
        let raw = self
            .must(
                &["diff", "--cached", "--raw", "-z", "--no-abbrev", base],
                None,
                None,
            )
            .await?;
        if raw.is_empty() {
            return Ok(Some("the patch changes nothing".to_string()));
        }
        if let Some(why) = refusal(&raw, policy) {
            return Ok(Some(why));
        }
        if let Some(why) = self.rust_refusal(&raw, policy).await? {
            return Ok(Some(why));
        }
        let tree = self.must(&["ls-files", "-z"], None, None).await?;
        Ok(case_collision(&raw, &tree))
    }

    /// What `raw`'s `.rs` files add of [`Policy::refused_in_rust`], read from
    /// the blobs git made and not the patch, which may carry them as binary.
    async fn rust_refusal(&self, raw: &[u8], policy: &Policy) -> Result<Option<String>> {
        if policy.refused_in_rust.is_empty() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(raw);
        let Ok(all) = touched(&text) else {
            return Ok(None);
        };
        let rust = |t: &&Touched<'_>| {
            t.new_mode != "000000" && t.path.to_ascii_lowercase().ends_with(".rs")
        };
        for t in all.iter().filter(rust) {
            let old = if t.old_mode == "000000" {
                Vec::new()
            } else {
                self.must(&["cat-file", "blob", t.old_id], None, None)
                    .await?
            };
            let new = self
                .must(&["cat-file", "blob", t.new_id], None, None)
                .await?;
            for pattern in &policy.refused_in_rust {
                if adds_text(&old, &new, pattern) {
                    return Ok(Some(format!("{} adds {pattern}", t.path)));
                }
            }
        }
        Ok(None)
    }

    /// Commits the index as ClaxBot and answers the commit; with nothing
    /// new in the index, the commit it already stands on, so that proposing
    /// again after a failed push or pull request finishes the job.
    pub async fn commit(&self, message: &str) -> Result<String> {
        let same = self
            .git(&["diff", "--cached", "--quiet", "HEAD"], None, None)
            .await?;
        if same.ok {
            return self.head().await;
        }
        self.must(
            &[
                "-c",
                "user.name=ClaxBot",
                "-c",
                &format!("user.email={BOT_EMAIL}"),
                "commit",
                "--quiet",
                "--no-verify",
                "--file=-",
            ],
            Some(message.as_bytes()),
            None,
        )
        .await?;
        self.head().await
    }

    /// The tree of `rev`.
    pub async fn tree_of(&self, rev: &str) -> Result<String> {
        let spec = format!("{rev}^{{tree}}");
        let out = self.must(&["rev-parse", &spec], None, None).await?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// The tree the index holds.
    pub async fn index_tree(&self) -> Result<String> {
        let out = self.must(&["write-tree"], None, None).await?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// What the index changes from `HEAD`, each file whole, for a forge to
    /// commit; `Ok(Err(why))` when it should not be sent. `apply_onto` has
    /// judged the index against the run's base, so this bounds what crosses
    /// and refuses what a forge's tree could not carry. Against `HEAD` a
    /// change can touch what the run's last proposal did as well as what
    /// this one does, so twice `max_files`.
    pub async fn tree_changes(&self, policy: &Policy) -> Result<Result<Vec<TreeChange>, String>> {
        let args = [
            "diff",
            "--cached",
            "--raw",
            "-z",
            "--no-abbrev",
            "--no-renames",
            "HEAD",
        ];
        let raw = self.must(&args, None, None).await?;
        let Ok(text) = std::str::from_utf8(&raw) else {
            return Ok(Err("a path is not UTF-8".to_string()));
        };
        let all = match touched(text) {
            Ok(all) => all,
            Err(why) => return Ok(Err(why)),
        };
        if all.len() > policy.max_files.saturating_mul(2) {
            return Ok(Err(format!("{} files is over the limit", all.len())));
        }
        let absent = "000000";
        let mut total: u64 = 0;
        for t in all.iter().filter(|t| t.new_mode != absent) {
            if !matches!(t.new_mode, "100644" | "100755") {
                return Ok(Err(format!("{} is not a plain file", t.path)));
            }
            let size = self.must(&["cat-file", "-s", t.new_id], None, None).await?;
            let size = String::from_utf8_lossy(&size).trim().parse::<u64>();
            total = total.saturating_add(size.context("a blob's size")?);
        }
        if total > policy.max_change_bytes {
            return Ok(Err(format!("{total} bytes of files is over the limit")));
        }
        let mut changes = Vec::with_capacity(all.len());
        for t in &all {
            let deleted = t.new_mode == absent;
            let blob = if deleted {
                None
            } else {
                let args = ["cat-file", "blob", t.new_id];
                Some(self.must(&args, None, None).await?)
            };
            changes.push(TreeChange {
                path: t.path.to_string(),
                mode: (if deleted { t.old_mode } else { t.new_mode }).to_string(),
                blob,
            });
        }
        Ok(Ok(changes))
    }

    /// Every path the index changes from where `HEAD` left `base`, a branch
    /// of the remote's: all a pull request of this branch into it holds.
    pub async fn changed_from(&self, base: &str) -> Result<Vec<String>> {
        let remote = format!("refs/remotes/origin/{base}");
        let fork = self
            .must(&["merge-base", "HEAD", &remote], None, None)
            .await?;
        let fork = String::from_utf8_lossy(&fork).trim().to_string();
        let args = [
            "diff",
            "--cached",
            "--name-only",
            "--no-renames",
            "-z",
            &fork,
        ];
        let out = self.must(&args, None, None).await?;
        Ok(String::from_utf8_lossy(&out)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Brings `refs/remotes/origin/<branch>` up to the remote, or removes it
    /// when the remote has no such branch: a forge moves a branch without
    /// this clone seeing, so [`Self::remote_tip`] holds only after this.
    pub async fn fetch_branch(&self, branch: &str, token: Option<&Token>) -> Result<()> {
        let heads = format!("refs/heads/{branch}");
        let remote = format!("refs/remotes/origin/{branch}");
        let found = self.reach(&["ls-remote", "origin", &heads], token).await?;
        if found.is_empty() {
            if self.remote_tip(branch).await?.is_some() {
                self.must(&["update-ref", "-d", &remote], None, None)
                    .await?;
            }
            return Ok(());
        }
        let spec = format!("+{heads}:{remote}");
        let args = [
            "-c",
            "gc.auto=0",
            "fetch",
            "--quiet",
            "--no-tags",
            "origin",
            &spec,
        ];
        self.reach(&args, token).await?;
        Ok(())
    }

    /// Pushes `branch` with `token`; a branch that moved under the run fails
    /// with [`PushRefused`] rather than being overwritten.
    pub async fn push(&self, branch: &str, token: Option<&Token>) -> Result<()> {
        let spec = format!("{branch}:{branch}");
        self.reach(&["push", "--quiet", "origin", &spec], token)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut v = Vec::new();
        for (meta, path) in entries {
            v.extend_from_slice(meta.as_bytes());
            v.push(0);
            v.extend_from_slice(path.as_bytes());
            v.push(0);
        }
        v
    }

    const EDIT: &str = ":100644 100644 aaa bbb M";

    #[test]
    fn an_ordinary_edit_passes() {
        let r = raw(&[(EDIT, "src/app.js")]);
        assert_eq!(refusal(&r, &Policy::default()), None);
    }

    #[test]
    fn workflows_attributes_and_submodule_lists_are_refused() {
        for path in [
            ".github/workflows/preview.yml",
            ".gitmodules",
            ".gitattributes",
        ] {
            let r = raw(&[(EDIT, path)]);
            assert!(
                refusal(&r, &Policy::default())
                    .unwrap()
                    .contains("protected")
            );
        }
    }

    #[test]
    fn attributes_and_lfs_settings_are_protected_at_any_depth_and_in_any_case() {
        for path in [
            ".lfsconfig",
            "vendor/.lfsconfig",
            "src/.gitattributes",
            "a/b/.gitattributes",
            "src/.GitAttributes",
        ] {
            let why = refusal(&raw(&[(EDIT, path)]), &Policy::default());
            assert!(why.is_some_and(|w| w.contains("protected")), "{path}");
        }
        let r = raw(&[(EDIT, "src/.gitattributes.md")]);
        assert_eq!(refusal(&r, &Policy::default()), None);
    }

    /// A policy of `writable` and `protected`, on top of the defaults.
    fn limited(writable: &[&str], protected: &[&str]) -> Policy {
        let mut p = Policy {
            writable_paths: writable.iter().map(ToString::to_string).collect(),
            ..Policy::default()
        };
        p.protected_paths
            .extend(protected.iter().map(ToString::to_string));
        p
    }

    #[test]
    fn text_is_new_only_if_there_is_more_of_it_than_there_was() {
        let pattern = "#[path";
        assert!(adds_text(b"", b"# [ path = \"x\" ]\nmod y;", pattern));
        assert!(adds_text(
            b"#[path=\"a\"]",
            b"#[path=\"a\"] #[path=\"b\"]",
            pattern
        ));
        assert!(!adds_text(
            b"#[path=\"a\"]",
            b"// moved\n#[path=\"a\"]",
            pattern
        ));
        assert!(!adds_text(b"", b"fn path() {}", pattern));
    }

    #[test]
    fn a_star_stands_for_one_segment_and_no_more() {
        let p = limited(&["crates/*/src/"], &[]);
        assert!(p.may_write("crates/clax-core/src/lib.rs"));
        assert!(p.may_write("crates/clax-core/src/deep/er/mod.rs"));
        assert!(!p.may_write("crates/clax-core/Cargo.toml"));
        assert!(!p.may_write("crates/clax-worker/vfs/src/lib.rs"));
        assert!(!p.may_write("crates/src/lib.rs"));
    }

    #[test]
    fn a_star_works_inside_a_segment_and_in_a_protected_name() {
        let p = limited(&["web/*.md"], &["**/*.lock", "web/vite.*.config.ts"]);
        assert!(p.may_write("web/notes.md"));
        assert!(!p.may_write("web/deep/notes.md"));
        assert!(p.protects("Cargo.lock") && p.protects("a/b/yarn.LOCK"));
        assert!(p.protects("web/vite.shell.config.ts"));
        assert!(!p.protects("web/vite.config.ts"));
    }

    #[test]
    fn no_writable_paths_set_no_limit() {
        let p = Policy::default();
        assert!(p.may_write("anywhere/at/all"));
        assert_eq!(refusal(&raw(&[(EDIT, "anywhere/at/all")]), &p), None);
    }

    #[test]
    fn a_path_outside_the_writable_paths_is_refused() {
        let p = limited(&["docs/"], &[]);
        assert_eq!(refusal(&raw(&[(EDIT, "docs/usage.md")]), &p), None);
        let why = refusal(&raw(&[(EDIT, "src/app.js")]), &p).unwrap();
        assert!(
            why.contains("src/app.js") && why.contains("outside"),
            "{why}"
        );
    }

    #[test]
    fn the_writable_paths_are_held_to_the_repositorys_own_case() {
        let p = limited(&["docs/"], &[]);
        assert!(!p.may_write("DOCS/usage.md"));
    }

    #[test]
    fn a_protected_path_inside_the_writable_paths_is_refused() {
        let p = limited(&["crates/*/src/"], &["**/build.rs", "crates/clax-bot/"]);
        for path in ["crates/a/src/build.rs", "crates/clax-bot/src/run.rs"] {
            let why = refusal(&raw(&[(EDIT, path)]), &p);
            assert!(why.is_some_and(|w| w.contains("protected")), "{path}");
        }
    }

    #[test]
    fn both_sides_of_a_rename_must_be_writable() {
        let p = limited(&["docs/"], &[]);
        let out = b":100644 100644 aaa bbb R100\0docs/a.md\0scripts/a.sh\0";
        assert!(refusal(out, &p).unwrap().contains("scripts/a.sh"));
        let inward = b":100644 100644 aaa bbb R100\0scripts/a.sh\0docs/a.md\0";
        assert!(refusal(inward, &p).unwrap().contains("scripts/a.sh"));
        let within = b":100644 100644 aaa bbb R100\0docs/a.md\0docs/b.md\0";
        assert_eq!(refusal(within, &p), None);
    }

    #[test]
    fn a_pattern_that_could_match_nothing_it_means_to_is_refused() {
        for bad in [
            "", "/etc/x", "a/**/b", "**/a/b", "a/../b", "a//b", "**", "**/",
        ] {
            assert!(check_pattern(bad).is_err(), "{bad:?}");
        }
        for good in [
            "a",
            "a/",
            "a/*/b/",
            "**/Cargo.toml",
            "**/*.lock",
            ".github/",
        ] {
            assert_eq!(check_pattern(good), Ok(()), "{good:?}");
        }
    }

    #[test]
    fn a_build_file_is_sensitive_whatever_a_target_lists() {
        let p = Policy::default();
        for path in [
            ".github/workflows/ci.yml",
            "Cargo.toml",
            "crates/a/Cargo.toml",
            "crates/a/build.rs",
            "Cargo.lock",
            "web/package.json",
            "web/pnpm-lock.yaml",
            "deploy/Dockerfile",
            "justfile",
            "x/BUILD.bazel",
        ] {
            assert!(p.sensitive(path), "{path}");
        }
        assert!(!p.sensitive("crates/a/src/lib.rs"));
    }

    #[test]
    fn a_path_a_target_refuses_is_sensitive_too() {
        let p = limited(&["docs/"], &["docs/secret.md"]);
        assert!(p.sensitive("docs/secret.md") && p.sensitive("src/lib.rs"));
        assert!(!p.sensitive("docs/usage.md"));
    }

    #[test]
    fn a_similarly_named_path_is_not_protected() {
        let r = raw(&[(EDIT, ".github-notes.md")]);
        assert_eq!(refusal(&r, &Policy::default()), None);
    }

    #[test]
    fn a_rename_out_of_a_protected_path_is_refused_by_either_name() {
        let r = b":100644 100644 aaa bbb R100\0.github/ci.yml\0ci.yml\0";
        assert!(refusal(r, &Policy::default()).is_some());
        let r = b":100644 100644 aaa bbb R100\0ci.yml\0.github/ci.yml\0";
        assert!(refusal(r, &Policy::default()).is_some());
    }

    #[test]
    fn symlinks_are_refused() {
        let r = raw(&[(":000000 120000 aaa bbb A", "link")]);
        assert!(refusal(&r, &Policy::default()).unwrap().contains("symlink"));
    }

    #[test]
    fn too_many_files_are_refused() {
        let p = Policy {
            max_files: 1,
            ..Policy::default()
        };
        let r = raw(&[(EDIT, "a"), (EDIT, "b")]);
        assert!(refusal(&r, &p).unwrap().contains("limit"));
    }

    #[test]
    fn a_mode_change_is_refused_and_a_new_or_deleted_executable_is_not() {
        let changed = raw(&[(":100644 100755 aaa bbb M", "tool.sh")]);
        assert!(
            refusal(&changed, &Policy::default())
                .unwrap()
                .contains("mode")
        );
        let added = raw(&[(":000000 100755 aaa bbb A", "tool.sh")]);
        assert_eq!(refusal(&added, &Policy::default()), None);
        let deleted = raw(&[(":100755 000000 aaa bbb D", "tool.sh")]);
        assert_eq!(refusal(&deleted, &Policy::default()), None);
    }

    #[test]
    fn a_path_differing_only_by_case_is_refused() {
        let r = raw(&[(":000000 100644 aaa bbb A", "Readme.md")]);
        assert!(case_collision(&r, b"README.md\0Readme.md\0").is_some());
        assert_eq!(case_collision(&r, b"Readme.md\0other\0"), None);
    }

    #[test]
    fn binary_sizes_add_up_and_a_text_line_is_not_a_hunk() {
        let p = Policy {
            max_binary_bytes: 100,
            ..Policy::default()
        };
        let two = b"literal 60\nabc\n\nliteral 60\nabc\n";
        assert!(patch_refusal(two, &p).unwrap().contains("limit"));
        assert_eq!(patch_refusal(b"literal 60\nabc\n", &p), None);
        assert_eq!(patch_refusal(b"+literal 999\n-delta 1\n", &p), None);
    }

    #[test]
    fn a_size_git_would_read_oddly_is_refused() {
        for size in ["-1", "0x10", "", "12 34", "99999999999999999999999"] {
            let patch = format!("literal {size}\n");
            assert!(
                patch_refusal(patch.as_bytes(), &Policy::default()).is_some(),
                "{size}"
            );
        }
    }

    #[test]
    fn a_token_is_redacted_as_itself_and_as_credentials() {
        let basic = basic_credentials("t0ken");
        let text = format!("saw t0ken and Basic {basic}");
        assert_eq!(
            redact(&text, ["t0ken"]),
            "saw [redacted] and Basic [redacted]"
        );
        assert_eq!(redact("text", [""]), "text");
    }

    #[test]
    fn a_redactor_remembers_what_was_issued() {
        let r = Redactor::default();
        r.note(&Token::new("first"));
        r.note(&Token::new("second"));
        assert_eq!(r.clean("first second"), "[redacted] [redacted]");
    }

    #[test]
    fn a_tokens_debug_shows_nothing_of_it() {
        assert!(!format!("{:?}", Token::new("s3cret")).contains("s3cret"));
        let access = StaticAccess {
            read: Some(Token::new("s3cret")),
            write: None,
        };
        assert!(!format!("{access:?}").contains("s3cret"));
    }

    #[test]
    fn a_token_env_refuses_redirects() {
        let env = token_env(&Token::new("t"));
        assert_eq!(env["GIT_CONFIG_COUNT"], "2");
        assert_eq!(env["GIT_CONFIG_KEY_1"], "http.followRedirects");
        assert_eq!(env["GIT_CONFIG_VALUE_1"], "false");
    }
}

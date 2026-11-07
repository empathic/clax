//! Pull requests, and on a forge that can sign, commits. Pull requests are
//! opened and never merged: there is no call for it here.
//!
//! A commit made by `git` on the host is unsigned. GitHub documents that it
//! signs a commit made through its Commits API by a request authenticated as
//! a GitHub App, carrying no custom author, committer or signature, and marks
//! it verified. It documents no such thing for a personal access token, so a
//! token is taken to give unsigned commits, and goes by `git push`.

use crate::repo::{Redactor, Remembered, RepoAccess, Token};
use crate::runtime::{bounded, control_client};
use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// The most of a forge's answer that is read.
const MAX_ANSWER: usize = 1024 * 1024;

/// How many times a commit not yet verified is read again, and how long
/// before each: GitHub documents no moment at which `verification` is
/// settled.
pub const VERIFY_ATTEMPTS: u32 = 3;
const VERIFY_DELAY: Duration = Duration::from_secs(2);

/// The most paths a pull request's text lists.
const MAX_LISTED: usize = 100;

/// The most paths the loud line names, how much of a path is shown, and how
/// much of a model's quoted text is kept, in bytes. With [`MAX_LISTED`] they
/// keep a pull request's text well under the 65536 GitHub takes, so nothing
/// pushed can then be refused for its text.
const MAX_FLAGGED: usize = 20;
const MAX_SPAN: usize = 160;
const MAX_SUMMARY: usize = 20_000;

/// How many pull requests a listing's page holds, and how many pages are read:
/// past that many open pull requests, a cap has long been reached.
const PAGE: usize = 100;
const MAX_PAGES: usize = 10;

/// A pull request.
#[derive(Clone, Debug, Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub html_url: String,
}

/// A pull request to open.
#[derive(Clone, Debug)]
pub struct NewPullRequest {
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
}

/// Whether a target may publish a commit nobody signed. It has no default:
/// whoever builds a target says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsignedCommits {
    Refuse,
    Allow,
}

/// The forge will not take this, and asking again will not change that. The
/// text is for the log.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// A commit the forge made.
#[derive(Clone, Debug)]
pub struct Committed {
    pub id: String,
    /// That the forge says it is signed.
    pub verified: bool,
}

/// One path's change in a commit.
#[derive(Clone)]
pub struct TreeChange {
    pub path: String,
    /// The file's mode, `100644` or `100755`.
    pub mode: String,
    /// What it now holds; `None` deletes it.
    pub blob: Option<Vec<u8>>,
}

/// A commit for a forge to make.
#[derive(Clone)]
pub struct NewCommit {
    pub parent: String,
    /// The parent's tree, which `changes` are against.
    pub base_tree: String,
    /// The tree that was judged. Trees are named by their content, so the
    /// forge must arrive at this one.
    pub tree: String,
    pub changes: Vec<TreeChange>,
    pub message: String,
}

/// Where the repository's pull requests live.
#[async_trait::async_trait]
pub trait Forge: Send + Sync {
    /// The open pull request of branch `head`, if there is one.
    async fn find_open(&self, head: &str) -> Result<Option<PullRequest>>;

    async fn open(&self, pr: &NewPullRequest) -> Result<PullRequest>;

    /// The text of pull request `number` as it stands, a person's edits with it.
    async fn body(&self, _number: u64) -> Result<String> {
        Ok(String::new())
    }

    /// Replaces the text of pull request `number` with `body`, as it is given.
    async fn describe(&self, _number: u64, _body: &str) -> Result<()> {
        Ok(())
    }

    /// How many open pull requests stand on branches under `prefix`.
    async fn open_under(&self, _prefix: &str) -> Result<usize> {
        Ok(0)
    }

    /// Which repository of which forge this is, to tell it from another.
    fn repository(&self) -> String {
        String::new()
    }

    /// Whether commits go through [`Self::commit`] and [`Self::advance`]
    /// rather than `git push`.
    fn commits(&self) -> bool {
        false
    }

    /// Makes `commit`, on no branch yet. The caller decides whether it may
    /// stand on one, and remembers its ID before any branch does.
    async fn commit(&self, _commit: &NewCommit) -> Result<Committed> {
        bail!("this forge makes no commits")
    }

    /// Moves `branch` to `commit`, only ever forward, making it unless it
    /// `exists`.
    async fn advance(&self, _branch: &str, _commit: &str, _exists: bool) -> Result<()> {
        bail!("this forge makes no commits")
    }
}

/// `body` with its `@` mentions defused: a pull request body notifies
/// whoever it names, and a summary is model text over reporter text.
pub fn defuse_mentions(body: &str) -> String {
    body.replace('@', "@\u{200b}")
}

/// `text` with its HTML comments gone, one left open taking the rest with it:
/// a reviewer reads none of what a comment holds, and the forge keeps it.
pub fn strip_html_comments(text: &str) -> String {
    let mut text = text.to_string();
    while let Some(start) = text.find("<!--") {
        match text[start..].find("-->") {
            Some(length) => text.replace_range(start..start + length + 3, ""),
            None => text.truncate(start),
        }
    }
    text
}

/// The words a forge reads as closing the issue that follows them.
const CLOSING: [&str; 9] = [
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

/// Whether a forge reads `c`, the last of `before`, and the `next` after it as
/// an issue's number: `#1`, which `other/repo#1` ends in, or `GH-1`.
fn names_issue(before: &str, c: char, next: char) -> bool {
    let gh = || {
        let tail = before.as_bytes().last_chunk::<3>();
        tail.is_some_and(|t| t.eq_ignore_ascii_case(b"gh-"))
    };
    next.is_ascii_digit() && (c == '#' || (c == '-' && gh()))
}

/// `text` with a zero-width space after each character `after` picks, given
/// what is kept so far, that character and the next.
fn spaced(text: &str, after: impl Fn(&str, char, char) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        out.push(c);
        let next = text[i + c.len_utf8()..].chars().next().unwrap_or(' ');
        if after(&out, c, next) {
            out.push('\u{200b}');
        }
    }
    out
}

/// A commit's subject as a model wrote it, but for an issue's number, which
/// the forge would link and a merge could act on. It is read as plain text
/// and searched, and must stay a conventional commit, so nothing else changes.
pub fn commit_subject(text: &str) -> String {
    spaced(text, names_issue)
}

/// Whether the word at `words[0]`, a closing word's next, is an address, or
/// opens a link to one on its line.
fn is_address(words: &[&str]) -> bool {
    let Some(first) = words.iter().find(|w| !w.trim().is_empty()) else {
        return false;
    };
    let link = |w: &&&str| w.to_ascii_lowercase().contains("](http");
    let line = || words.iter().take_while(|w| !w.contains('\n'));
    first.to_ascii_lowercase().starts_with("http")
        || (first.starts_with('[') && line().any(|w| link(&w)))
}

/// `text` with nothing in it a forge acts on, by a zero-width space where it
/// would: raw HTML, an alert's `[!`, which a quoted line would turn into a box
/// like the host's, an issue's number in any form (`#1`, `other/repo#1`,
/// `GH-1`), and a closing word before an issue's address, or against one.
pub fn neutralize(text: &str) -> String {
    let out = spaced(text, |before, c, next| {
        let tag = next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?');
        names_issue(before, c, next) || (c == '<' && tag) || (c == '[' && next == '!')
    });
    let words: Vec<&str> = out.split_inclusive(char::is_whitespace).collect();
    let mut done = String::with_capacity(out.len());
    for (i, word) in words.iter().enumerate() {
        let bare = word.trim_end().to_ascii_lowercase();
        let (head, glued) = bare.split_once(':').unwrap_or((&bare, ""));
        let closes = CLOSING.contains(&head)
            && (glued.starts_with("http") || (glued.is_empty() && is_address(&words[i + 1..])));
        if closes {
            done.push_str(&word[..1]);
            done.push('\u{200b}');
            done.push_str(&word[1..]);
        } else {
            done.push_str(word);
        }
    }
    done
}

/// `current`, a pull request's text, with its host block replaced by `block`
/// and all that follows left as it is. Only a [`HOST_END`] that starts a line
/// ends a block: a listed path may hold those words, and no path starts a
/// line. A text with no end to a block, which a person rewrote, goes below the
/// new one whole.
pub fn with_host_block(current: &str, block: &str) -> String {
    let end = format!("\n{HOST_END}");
    match current.find(&end) {
        Some(at) => format!("{block}{}", &current[at + end.len()..]),
        None => format!("{block}\n\n{current}"),
    }
}

/// Whether `c` is a format character, as a bidirectional override or a
/// zero-width one is: shown as nothing, or as something else.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// `text` cut to at most `most` bytes, on a character's edge.
fn cut(text: &str, most: usize) -> &str {
    let mut end = most.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `path` as inline code it cannot leave, whatever a model named a file.
fn code_span(path: &str) -> String {
    let safe = |c: char| {
        if c == '`' || c.is_control() || is_format(c) {
            '?'
        } else {
            c
        }
    };
    let shown: String = path.chars().map(safe).collect();
    let more = if shown.len() > MAX_SPAN { "…" } else { "" };
    format!("`{}{more}`", cut(&shown, MAX_SPAN))
}

/// Where the host's part of a pull request's text ends, on a line of its own.
/// A model cannot write that: comments are stripped from its text, every line
/// of that is quoted, and a path it names is inline code on a listed line.
pub const HOST_END: &str = "<!-- clax:host-end -->";

/// The host's part of a pull request's text: `paths` are all it changes,
/// `flagged` those a person should look at before anything of it runs, and
/// `report` how to name the report it came from.
pub fn host_block(paths: &[String], flagged: &[&String], report: &str) -> String {
    let mut body = String::new();
    if !flagged.is_empty() {
        let named: Vec<String> = flagged
            .iter()
            .take(MAX_FLAGGED)
            .map(|p| code_span(p))
            .collect();
        let more = flagged.len().saturating_sub(MAX_FLAGGED);
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        body.push_str(&format!(
            "> [!CAUTION]\n> **This touches what builds, deploys or decides who may do what:** \
             {}{more}\n> Read those first: nothing of this should run until a person has.\n\n",
            named.join(", ")
        ));
    }
    body.push_str(&format!("**Changed paths ({}):**\n\n", paths.len()));
    for path in paths.iter().take(MAX_LISTED) {
        body.push_str(&format!("- {}\n", code_span(path)));
    }
    if paths.len() > MAX_LISTED {
        body.push_str(&format!("- and {} more\n", paths.len() - MAX_LISTED));
    }
    body.push_str(&format!(
        "\nFrom report {report}. Written by ClaxBot; a person reviews and merges. The host wrote \
         all of this, and rewrites it on every push. What is quoted below the line is the \
         model's, of the first push alone, and a report's words steered it.\n{HOST_END}"
    ));
    body
}

/// A model's `summary` as a pull request carries it: below a line, every line
/// quoted, so that nothing of it can pose as the host's, with nothing in it a
/// forge acts on, and no longer than [`MAX_SUMMARY`]. It is cut to that first
/// too: a model sets its length, and proposals wait on this one.
fn model_part(summary: &str) -> String {
    let read = cut(summary, MAX_SUMMARY);
    let text = neutralize(&strip_html_comments(read)).replace('\r', "\n");
    let quoted: String = text.lines().map(|l| format!("> {l}\n")).collect();
    let kept = cut(&quoted, MAX_SUMMARY);
    let note = if kept.len() < quoted.len() || read.len() < summary.len() {
        "\n> (cut here: it was too long)\n"
    } else {
        ""
    };
    format!("\n\n---\n\n{kept}{note}")
}

/// What a pull request says of itself when it opens: [`host_block`], then the
/// model's `summary`.
pub fn pull_request_body(
    paths: &[String],
    flagged: &[&String],
    report: &str,
    summary: &str,
) -> String {
    format!(
        "{}{}",
        host_block(paths, flagged, report),
        model_part(summary)
    )
}

/// GitHub's REST API, or anything serving the same two calls. Its token
/// comes from `access`, the write token, for each call.
#[derive(Clone)]
pub struct GitHub {
    pub api_url: String,
    /// `owner/name`.
    pub repo: String,
    pub access: Arc<dyn RepoAccess>,
    pub http: reqwest::Client,
    /// See [`Forge::commits`].
    pub commits: bool,
    /// The tokens this forge used, kept out of what it quotes, whatever
    /// `access` was given.
    seen: Arc<Redactor>,
    verify_delay: Duration,
}

/// An object the forge made.
#[derive(Deserialize)]
struct Made {
    sha: String,
}

/// A pull request's text, which GitHub answers as null when there is none.
#[derive(Deserialize)]
struct Described {
    #[serde(default)]
    body: Option<String>,
}

/// A pull request as a listing shows it, for what it stands on.
#[derive(Deserialize)]
struct Listed {
    #[serde(default)]
    head: Option<ListedHead>,
}

#[derive(Deserialize)]
struct ListedHead {
    #[serde(rename = "ref")]
    name: String,
}

/// What GitHub says of a commit's signature.
#[derive(Default, Deserialize)]
struct Verification {
    verified: bool,
    reason: Option<String>,
}

#[derive(Deserialize)]
struct MadeCommit {
    sha: String,
    #[serde(default)]
    verification: Verification,
}

impl GitHub {
    pub fn new(api_url: &str, repo: &str, access: Arc<dyn RepoAccess>) -> GitHub {
        let seen = Arc::new(Redactor::default());
        GitHub {
            api_url: api_url.to_string(),
            repo: repo.to_string(),
            access: Arc::new(Remembered {
                inner: access,
                redactor: seen.clone(),
            }),
            http: control_client(),
            commits: false,
            seen,
            verify_delay: VERIFY_DELAY,
        }
    }

    /// This forge, making commits itself. Only an App's token gets them
    /// signed.
    #[must_use]
    pub fn committing(self) -> GitHub {
        GitHub {
            commits: true,
            ..self
        }
    }

    /// This forge, making its calls with `http`.
    #[must_use]
    pub fn with_http(self, http: reqwest::Client) -> GitHub {
        GitHub { http, ..self }
    }

    /// This forge, waiting `delay` before reading a commit again.
    #[must_use]
    pub fn with_verify_delay(self, delay: Duration) -> GitHub {
        GitHub {
            verify_delay: delay,
            ..self
        }
    }

    /// The Git Data route `path`.
    fn git(&self, path: &str) -> String {
        format!(
            "{}/repos/{}/git/{path}",
            self.api_url.trim_end_matches('/'),
            self.repo
        )
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        let request = self.http.post(self.git(path)).json(&body);
        self.call(request, &format!("posting to git/{path}")).await
    }

    /// Whether `sha` comes to be verified within [`VERIFY_ATTEMPTS`] reads.
    async fn verified_later(&self, sha: &str) -> bool {
        for _ in 0..VERIFY_ATTEMPTS {
            tokio::time::sleep(self.verify_delay).await;
            let request = self.http.get(self.git(&format!("commits/{sha}")));
            match self.call::<MadeCommit>(request, "reading a commit").await {
                Ok(c) if c.verification.verified => return true,
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "reading a commit again failed"),
            }
        }
        false
    }

    fn pulls(&self) -> String {
        format!(
            "{}/repos/{}/pulls",
            self.api_url.trim_end_matches('/'),
            self.repo
        )
    }

    /// `request` with the write token. A token the forge refuses is
    /// reported, for the next call to get another.
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        what: &str,
    ) -> Result<T> {
        let token: Token = self
            .access
            .write_token()
            .await?
            .context("no token for the forge")?;
        let r = request
            .bearer_auth(token.expose())
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", "clax-bot-workspace")
            .send()
            .await
            .with_context(|| what.to_string())?;
        if r.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.access.write_refused(&token).await;
        }
        answer(r, &self.seen).await
    }

    fn owner(&self) -> &str {
        self.repo.split('/').next().unwrap_or(&self.repo)
    }
}

/// Whether `status` says the same request would fare no better. A token gone
/// stale, a timeout and a rate limit may.
fn is_refusal(status: reqwest::StatusCode) -> bool {
    status.is_client_error() && !matches!(status.as_u16(), 401 | 408 | 429)
}

async fn answer<T: serde::de::DeserializeOwned>(
    r: reqwest::Response,
    seen: &Redactor,
) -> Result<T> {
    let status = r.status();
    let body = bounded(r, MAX_ANSWER)
        .await
        .context("reading the forge's answer")?
        .context("the forge's answer is over the limit")?;
    if !status.is_success() {
        // Whole: what redacts it must see any token it quotes whole.
        let text = seen.clean(&String::from_utf8_lossy(&body));
        let text = format!("the forge answered {status}: {text}");
        if is_refusal(status) {
            return Err(Refused(text).into());
        }
        bail!("{text}");
    }
    serde_json::from_slice(&body).context("the forge's answer")
}

#[async_trait::async_trait]
impl Forge for GitHub {
    fn repository(&self) -> String {
        format!("{} {}", self.api_url.trim_end_matches('/'), self.repo)
    }

    async fn find_open(&self, head: &str) -> Result<Option<PullRequest>> {
        let request = self.http.get(self.pulls()).query(&[
            ("state", "open"),
            ("head", &format!("{}:{head}", self.owner())),
        ]);
        let found: Vec<PullRequest> = self.call(request, "listing pull requests").await?;
        Ok(found.into_iter().next())
    }

    async fn open(&self, pr: &NewPullRequest) -> Result<PullRequest> {
        let request = self.http.post(self.pulls()).json(&json!({
            "title": defuse_mentions(&pr.title),
            "body": defuse_mentions(&pr.body),
            "head": pr.head,
            "base": pr.base,
        }));
        self.call(request, "opening the pull request").await
    }

    async fn body(&self, number: u64) -> Result<String> {
        let request = self.http.get(format!("{}/{number}", self.pulls()));
        let found: Described = self.call(request, "reading the pull request").await?;
        Ok(found.body.unwrap_or_default())
    }

    async fn describe(&self, number: u64, body: &str) -> Result<()> {
        let url = format!("{}/{number}", self.pulls());
        let request = self.http.patch(url).json(&json!({"body": body}));
        let _: serde_json::Value = self.call(request, "changing the pull request").await?;
        Ok(())
    }

    async fn open_under(&self, prefix: &str) -> Result<usize> {
        let mut count = 0;
        for page in 1..=MAX_PAGES {
            let request = self.http.get(self.pulls()).query(&[
                ("state", "open".to_string()),
                ("per_page", PAGE.to_string()),
                ("page", page.to_string()),
            ]);
            let found: Vec<Listed> = self.call(request, "listing pull requests").await?;
            let under = |l: &&Listed| l.head.as_ref().is_some_and(|h| h.name.starts_with(prefix));
            count += found.iter().filter(under).count();
            if found.len() < PAGE {
                break;
            }
        }
        Ok(count)
    }

    fn commits(&self) -> bool {
        self.commits
    }

    /// No author, committer or signature: any of them loses GitHub's own.
    async fn commit(&self, commit: &NewCommit) -> Result<Committed> {
        let mut entries = Vec::with_capacity(commit.changes.len());
        for change in &commit.changes {
            let sha = match &change.blob {
                Some(bytes) => {
                    let content = base64::engine::general_purpose::STANDARD.encode(bytes);
                    let body = json!({"content": content, "encoding": "base64"});
                    Some(self.post::<Made>("blobs", body).await?.sha)
                }
                None => None,
            };
            entries.push(json!({
                "path": change.path,
                "mode": change.mode,
                "type": "blob",
                "sha": sha,
            }));
        }
        let body = json!({"base_tree": commit.base_tree, "tree": entries});
        let tree: Made = self.post("trees", body).await?;
        if tree.sha != commit.tree {
            let text = format!("the forge's tree {} is not the one judged", tree.sha);
            return Err(Refused(text).into());
        }
        let body = json!({
            "message": commit.message,
            "tree": tree.sha,
            "parents": [commit.parent],
        });
        let made: MadeCommit = self.post("commits", body).await?;
        let verified = made.verification.verified || self.verified_later(&made.sha).await;
        if !verified {
            let reason = made.verification.reason.as_deref().unwrap_or("none given");
            tracing::warn!(
                commit = made.sha,
                reason,
                "the forge did not verify a commit"
            );
        }
        Ok(Committed {
            id: made.sha,
            verified,
        })
    }

    async fn advance(&self, branch: &str, commit: &str, exists: bool) -> Result<()> {
        let request = if exists {
            let url = self.git(&format!("refs/heads/{branch}"));
            let body = json!({"sha": commit, "force": false});
            self.http.patch(url).json(&body)
        } else {
            let body = json!({"ref": format!("refs/heads/{branch}"), "sha": commit});
            self.http.post(self.git("refs")).json(&body)
        };
        let _: serde_json::Value = self.call(request, "moving the branch").await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comment_is_gone_wherever_it_stands() {
        assert_eq!(strip_html_comments("a<!-- b -->c"), "ac");
        assert_eq!(strip_html_comments("a<!-- b\nc\n-->d"), "ad");
        assert_eq!(strip_html_comments("no comment"), "no comment");
    }

    #[test]
    fn a_comment_left_open_takes_the_rest_with_it() {
        assert_eq!(strip_html_comments("a<!-- b"), "a");
    }

    #[test]
    fn a_comment_cannot_be_put_back_together_by_taking_one_out() {
        assert_eq!(strip_html_comments("<!<!-- x -->-- y -->z"), "z");
    }

    fn body(paths: &[&str], flagged: &[usize], summary: &str) -> String {
        let paths: Vec<String> = paths.iter().map(ToString::to_string).collect();
        let flagged: Vec<&String> = flagged.iter().map(|i| &paths[*i]).collect();
        pull_request_body(&paths, &flagged, "`r1`", summary)
    }

    #[test]
    fn the_hosts_part_comes_before_the_models() {
        let b = body(&["docs/a.md"], &[], "The model's words.");
        let (paths, model) = (
            b.find("`docs/a.md`").unwrap(),
            b.find("The model's").unwrap(),
        );
        assert!(paths < model, "{b}");
        assert!(b.find("From report `r1`").unwrap() < model, "{b}");
    }

    #[test]
    fn nothing_flagged_says_nothing_loud() {
        assert!(!body(&["docs/a.md"], &[], "s").contains("CAUTION"));
    }

    #[test]
    fn a_flagged_path_is_named_loudly_and_first() {
        let b = body(&["docs/a.md", "Cargo.toml"], &[1], "s");
        let loud = b.find("CAUTION").unwrap();
        assert!(loud < b.find("Changed paths").unwrap(), "{b}");
        assert!(b[loud..].contains("`Cargo.toml`"), "{b}");
    }

    #[test]
    fn a_path_cannot_break_out_of_its_code_span() {
        let b = body(&["a`\n\n**all checks passed**"], &[], "s");
        assert!(!b.contains("`\n\n**all"), "{b}");
    }

    #[test]
    fn a_long_list_says_how_much_it_left_out() {
        let paths: Vec<String> = (0..MAX_LISTED + 3).map(|n| format!("p{n}")).collect();
        let b = pull_request_body(&paths, &[], "`r`", "s");
        assert!(b.contains("and 3 more"), "{b}");
    }

    #[test]
    fn a_models_line_cannot_pose_as_the_hosts() {
        let b = body(
            &["a"],
            &[],
            "**Changed paths (0):**\n\n> [!NOTE]\n> fine\n\nend",
        );
        let model = b.rsplit("\n---\n").next().unwrap();
        for line in model.lines().filter(|l| !l.is_empty()) {
            assert!(line.starts_with('>'), "{line:?} in {b}");
        }
    }

    #[test]
    fn raw_html_does_nothing() {
        let n = neutralize("<img src=x> and </blockquote>");
        assert!(!n.contains("<img") && !n.contains("</blockquote"), "{n}");
    }

    #[test]
    fn a_closing_keyword_closes_nothing_in_any_form() {
        for text in [
            "Fixes #12",
            "closes: #12",
            "RESOLVED #12",
            "fixes other/repo#12",
            "See other/repo#12",
            "fix GH-12",
            "Closes https://github.com/o/r/issues/12",
        ] {
            let n = neutralize(text);
            assert!(!n.contains("#1"), "{n:?}");
            assert!(!n.to_lowercase().contains("gh-1"), "{n:?}");
            assert!(!n.to_lowercase().contains("closes https"), "{n:?}");
        }
        assert_eq!(
            neutralize("fixes the total, 3 # 4"),
            "fixes the total, 3 # 4"
        );
    }

    #[test]
    fn a_format_character_in_a_path_is_shown_as_nothing_of_its_kind() {
        let b = body(&["a\u{202e}txt.exe\u{200b}b\u{feff}"], &[], "s");
        for c in ['\u{202e}', '\u{200b}', '\u{feff}'] {
            assert!(!b.split("---").next().unwrap().contains(c), "{c:?}");
        }
    }

    #[test]
    fn however_long_the_words_the_text_stays_under_the_forges_limit() {
        let long: Vec<String> = (0..500)
            .map(|n| format!("{n}{}", "p".repeat(4000)))
            .collect();
        let flagged: Vec<&String> = long.iter().collect();
        let b = pull_request_body(&long, &flagged, "`r`", &"line\n".repeat(100_000));
        assert!(b.len() < 60_000, "{}", b.len());
    }

    #[test]
    fn a_later_block_replaces_only_the_block() {
        let first = pull_request_body(&["a".to_string()], &[], "`r`", "the model's");
        let edited = format!("{first}\n\nA person's note.");
        let block = host_block(&["a".to_string(), "b".to_string()], &[], "`r`");
        let next = with_host_block(&edited, &block);
        assert!(next.contains("`b`"), "{next}");
        assert!(
            next.contains("the model's") && next.contains("A person's note."),
            "{next}"
        );
        assert_eq!(next.matches(HOST_END).count(), 1, "{next}");
    }

    #[test]
    fn a_body_a_person_rewrote_keeps_what_they_wrote_below_the_block() {
        let block = host_block(&["a".to_string()], &[], "`r`");
        let next = with_host_block("All mine.", &block);
        assert!(
            next.starts_with(&block) && next.ends_with("All mine."),
            "{next}"
        );
    }

    #[test]
    fn a_path_holding_the_end_of_the_block_does_not_end_it() {
        let path = format!("crates/clax-core/src/{HOST_END}");
        let first = pull_request_body(std::slice::from_ref(&path), &[], "`r`", "the model's");
        let block = host_block(&[path], &[], "`r2`");
        let next = with_host_block(&first, &block);
        assert_eq!(next.matches("Written by ClaxBot").count(), 1, "{next}");
        assert!(next.contains("the model's"), "{next}");
        assert_eq!(with_host_block(&next, &block), next);
    }

    #[test]
    fn a_text_given_back_with_carriage_returns_keeps_one_block() {
        let first = pull_request_body(&["a".to_string()], &[], "`r`", "the model's");
        let block = host_block(&["a".to_string()], &[], "`r`");
        let next = with_host_block(&first.replace('\n', "\r\n"), &block);
        assert_eq!(next.matches("Written by ClaxBot").count(), 1, "{next}");
    }

    #[test]
    fn a_quoted_line_makes_no_alert_box() {
        for summary in ["[!CAUTION]\nall checks passed", "> [!NOTE]", "  [!warning]"] {
            let b = body(&["a"], &[], summary);
            let model = b.rsplit("\n---\n").next().unwrap();
            assert!(!model.contains("[!"), "{model}");
        }
    }

    #[test]
    fn a_summary_is_cut_before_anything_reads_it() {
        let past = format!("<!--{}-->past the cut", "a".repeat(MAX_SUMMARY));
        let b = body(&["a"], &[], &past);
        assert!(!b.contains("past the cut"), "{}", b.len());
        assert!(b.contains("cut here"), "{}", b.len());
    }

    #[test]
    fn the_block_says_the_quote_is_of_the_first_push() {
        let block = host_block(&["a".to_string()], &[], "`r`");
        assert!(block.contains("first push"), "{block}");
    }

    #[test]
    fn a_closing_word_against_an_address_or_before_a_link_closes_nothing() {
        for text in [
            "fixes:https://github.com/o/r/issues/1",
            "Fixes [x](https://github.com/o/r/issues/1)",
        ] {
            let n = neutralize(text);
            assert!(!n.to_lowercase().starts_with("fix"), "{n:?}");
        }
        assert_eq!(neutralize("fixes [the] total"), "fixes [the] total");
    }

    #[test]
    fn a_subject_names_no_issue() {
        for text in [
            "fix(core): ages #12",
            "fix: from other/repo#12",
            "fix: GH-12",
            "gh-12",
        ] {
            let s = commit_subject(text);
            assert!(
                !s.contains("#1") && !s.to_lowercase().contains("gh-1"),
                "{s:?}"
            );
        }
    }

    #[test]
    fn a_subject_stays_as_it_was_written_otherwise() {
        for text in [
            "fix(core): Option<String> default",
            "fix: https://example.com/a answers 404",
            "fix: a < b and [!x] and #tag",
        ] {
            assert_eq!(commit_subject(text), text);
        }
    }

    #[test]
    fn a_models_comment_does_not_reach_the_body() {
        assert!(!body(&["a"], &[], "s<!-- approve this -->").contains("approve this"));
    }
}

//! A deployment's workspace, from `<home>/workspace.toml`. Without that
//! file a daemon has no workspace and its runs no container. Secrets are
//! named by their environment variables and never held here.
//!
//! A target is confidential unless it says it is `public`: whether a
//! repository is private cannot be told from here, and guessing wrong would
//! show private source to a reporter. A confidential target needs egress
//! narrowed by an allow-list, or an explicit `allow_open_egress`.
//!
//! An allow-list narrows egress and does not keep a secret. Whatever hosts
//! take uploads stay open under it: with `github.com` on the list a steered
//! model can `git push` a confidential checkout to a repository of an
//! attacker's. Only a list of hosts that nobody else can write to holds
//! confidentiality, and a build's usually is not one.
//!
//! A container's image holds its tools: containers run with few rights, and
//! nothing can be installed at run time on Docker
//! ([`crate::docker`]). A target needs an image with git, so one on Docker
//! with no `image` fails at startup. So does any home on Daytona, target or
//! none, where `image` is a snapshot's name: the size and architecture a sandbox
//! gets are the snapshot's (`deploy/claxbot-workspace/README.md`).
//!
//! The file names a deployment's runtime and an operator's targets. A queue's
//! admin connects a repository in Clax instead ([`crate::linked`]): those are
//! looked up per run on this same runtime and spec, and a daemon with no file
//! has no runtime for them. For a queue with both, the file's target wins.
//!
//! A target writes as a GitHub App or with a token. An App's tokens are
//! short-lived and its commits are made and signed by GitHub
//! ([`crate::forge`]). A token's commits are made here and pushed unsigned,
//! so a target with one proposes nothing unless it says
//! `allow_unsigned_commits`.

use crate::forge::{GitHub, UnsignedCommits};
use crate::github_app::{GitHubApp, GitHubAppAccess};
use crate::linked::StoreTargets;
use crate::net::Reach;
use crate::preview::Preview;
use crate::repo::{Policy, Redactor, RepoAccess, StaticAccess, Token, check_pattern};
use crate::runtime::{ContainerSpec, RuntimeConfig};
use crate::workspace::{ContainerWorkspace, LABEL_WORKSPACE, Target};
use anyhow::{Context, Result, bail};
use clax_bot::workspace::{MAX_GUIDANCE_BYTES, MAX_PREVIEW_GATE_BYTES};
use clax_store::Store;

use clax_store::secrets::Kek;
use reqwest::Url;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Where a daemon's store links come from, and the key that opens them.
pub struct Links {
    pub store: Arc<Store>,
    pub kek: Kek,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub runtime: RuntimeConfig,
    /// The image containers start from, or on Daytona the snapshot's name.
    pub image: Option<String>,
    /// Idle minutes before the runtime stops a container. At least
    /// [`MIN_STOP_MINUTES`]: a command may run for ten, and the idle rule
    /// must not stop a container under it.
    #[serde(default = "default_stop")]
    pub auto_stop_minutes: u32,
    #[serde(default)]
    pub domain_allow_list: Vec<String>,
    /// Whether a run with no target gets a bare container.
    #[serde(default)]
    pub shell_for_all: bool,
    #[serde(default, rename = "target")]
    pub targets: Vec<TargetConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConfig {
    /// The queue whose reports work on this repository.
    pub queue: String,
    pub repo_url: String,
    /// Where a container clones from, when that is another address.
    pub container_repo_url: Option<String>,
    #[serde(default = "default_base")]
    pub base: String,
    /// `owner/name`.
    pub repo: String,
    #[serde(default = "default_github")]
    pub forge_api_url: String,
    /// Holds a token that may push and open pull requests. A target has
    /// this or `github_app`.
    pub write_token_env: Option<String>,
    /// Holds a read-only token for the container's clone; a public
    /// repository needs none. Never the write token.
    pub read_token_env: Option<String>,
    /// That a target may publish commits nobody signed, as a demo's or a
    /// local forge's does. This sets [`Target::unsigned`] and no more.
    #[serde(default)]
    pub allow_unsigned_commits: bool,
    /// The App this target's tokens and commits come from, in place of the
    /// token variables.
    pub github_app: Option<GitHubAppConfig>,
    /// That the repository may be shown to anyone. Without it a target is
    /// confidential.
    #[serde(default)]
    pub public: bool,
    /// That a confidential target may run with egress no allow-list holds.
    #[serde(default)]
    pub allow_open_egress: bool,
    /// That each of the target's deploys needs a person's approval of the
    /// commit, as `.github/workflows/preview.yml` here does.
    #[serde(default)]
    pub reviewed_previews: bool,
    /// That a reporter's plain reply to a waiting fix starts a run that may
    /// answer it. Without it only a reopen starts one.
    #[serde(default)]
    pub wake_on_reply: bool,
    /// That a reporter's word on a fix may start a run with no person sending
    /// it. A public repository's target says `false`: what a run reads there
    /// is a reporter's text, and a person's Send is then its review.
    #[serde(default = "default_true")]
    pub follow_ups: bool,
    /// On top of the defaults of [`Policy`]. [`crate::repo::check_pattern`]
    /// says what an entry may be.
    #[serde(default)]
    pub protected_paths: Vec<String>,
    /// Where a change may write: [`Policy::writable_paths`]. Empty sets no
    /// limit, and `protected_paths` subtract from it.
    #[serde(default)]
    pub writable_paths: Vec<String>,
    /// [`Policy::refused_in_rust`].
    #[serde(default)]
    pub refused_in_rust: Vec<String>,
    /// [`Policy::max_open_pull_requests`], when not its default.
    pub max_open_pull_requests: Option<usize>,
    /// [`Policy::max_pushes_per_day`], when not its default.
    pub max_pushes_per_day: Option<usize>,
    /// [`Policy::report_url`].
    pub report_url: Option<String>,
    /// [`Policy::guidance`]. At most [`MAX_GUIDANCE_BYTES`].
    pub guidance: Option<String>,
    /// [`Policy::preview_gate`]. At most [`MAX_PREVIEW_GATE_BYTES`].
    pub preview_gate: Option<String>,
    pub preview_url: String,
    #[serde(default = "default_ready")]
    pub preview_ready_path: String,
    #[serde(default = "default_true")]
    pub preview_names_commit: bool,
    #[serde(default = "default_preview_secs")]
    pub preview_timeout_secs: u64,
    #[serde(default = "default_poll_secs")]
    pub preview_poll_secs: u64,
}

/// A GitHub App's installation on a target's repository.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubAppConfig {
    pub app_id: u64,
    pub installation_id: u64,
    /// Holds the App's private key, as the PEM GitHub issues. It stays in
    /// the daemon.
    pub private_key_env: String,
}

/// The least of `auto_stop_minutes`.
pub const MIN_STOP_MINUTES: u32 = 15;

fn default_stop() -> u32 {
    30
}
fn default_base() -> String {
    "main".into()
}
fn default_github() -> String {
    "https://api.github.com".into()
}
fn default_ready() -> String {
    "/healthz".into()
}
fn default_true() -> bool {
    true
}
fn default_preview_secs() -> u64 {
    3600
}
fn default_poll_secs() -> u64 {
    10
}

fn secret(name: &str) -> Result<Token> {
    let value = std::env::var(name).with_context(|| format!("{name} is not set"))?;
    Ok(Token::new(value))
}

/// Refuses what cannot carry a token as this crate sends one, or would send
/// it in the clear, or holds its own. A token goes in an HTTP header, so
/// there is no ssh here. A path carries no token anywhere.
fn check_url(queue: &str, url: &str) -> Result<()> {
    let Ok(parsed) = Url::parse(url) else {
        // `host:path` and `user@host:path` are git's ssh; `a/b:c` is a path.
        let before_slash = url.split('/').next().unwrap_or_default();
        if before_slash.contains(':') {
            bail!("target {queue}: ssh URLs are not supported: use https");
        }
        return Ok(());
    };
    if !matches!(parsed.scheme(), "https" | "http" | "file") {
        bail!(
            "target {queue}: ssh and other {} URLs are not supported: use https",
            parsed.scheme()
        );
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("target {queue}: a URL may not hold credentials: use the token variables");
    }
    let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if parsed.scheme() == "http" && !local {
        bail!("target {queue}: a token may not go to plain http: {}", url);
    }
    Ok(())
}

/// Refuses an entry that would allow everything or nothing, or would be two
/// entries once the list is joined with commas.
fn check_domain(entry: &str) -> Result<()> {
    let named = entry.chars().any(|c| !matches!(c, '*' | '.' | ' '));
    if !named || entry.contains(',') {
        bail!("domain_allow_list: {entry:?} does not name a host");
    }
    Ok(())
}

/// This home's own value of [`LABEL_WORKSPACE`], made the first time and
/// kept, so that two daemons sharing a runtime tell their containers apart.
fn deployment_id(home: &Path) -> Result<String> {
    let path = home.join("workspace-id");
    match std::fs::read_to_string(&path) {
        Ok(id) if !id.trim().is_empty() => return Ok(id.trim().to_string()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let id = format!("{nanos:x}-{:x}", std::process::id());
    std::fs::write(&path, &id).with_context(|| format!("writing {}", path.display()))?;
    Ok(id)
}

impl TargetConfig {
    /// Whether a run on it holds confidential material.
    pub fn is_confidential(&self) -> bool {
        !self.public
    }

    fn check(&self) -> Result<()> {
        let q = &self.queue;
        if !self.preview_url.contains("{pr}") {
            bail!("target {q}: preview_url names no {{pr}}");
        }
        self.check_credentials()?;
        if self.read_token_env.is_some() && self.public {
            bail!("target {q}: a public repository takes no read_token_env");
        }
        for pattern in self.protected_paths.iter().chain(&self.writable_paths) {
            check_pattern(pattern).map_err(|why| anyhow::anyhow!("target {q}: {why}"))?;
        }
        if let Some(url) = &self.report_url
            && !(url.starts_with("https://") && url.contains("{report}"))
        {
            bail!("target {q}: report_url is an https address naming {{report}}");
        }
        if self
            .guidance
            .as_ref()
            .is_some_and(|g| g.len() > MAX_GUIDANCE_BYTES)
        {
            bail!("target {q}: guidance is over {MAX_GUIDANCE_BYTES} bytes");
        }
        if self
            .preview_gate
            .as_ref()
            .is_some_and(|g| g.len() > MAX_PREVIEW_GATE_BYTES)
        {
            bail!("target {q}: preview_gate is over {MAX_PREVIEW_GATE_BYTES} bytes");
        }
        let gated = self.preview_gate.as_deref().map(str::trim);
        if self.reviewed_previews && gated.is_none_or(str::is_empty) {
            bail!(
                "target {q}: reviewed_previews needs a preview_gate, or a person who must deploy \
                 a preview hears nothing until its wait ends: set preview_gate to what they do, \
                 such as \"post `/preview {{commit}}` on the pull request\""
            );
        }
        check_url(q, &self.repo_url)?;
        check_url(q, &self.forge_api_url)?;
        if let Some(url) = &self.container_repo_url {
            check_url(q, url)?;
        }
        Ok(())
    }

    fn check_credentials(&self) -> Result<()> {
        let q = &self.queue;
        let Some(write) = &self.write_token_env else {
            if self.github_app.is_none() {
                bail!("target {q}: name a [target.github_app] or a write_token_env");
            }
            if self.read_token_env.is_some() {
                bail!("target {q}: github_app mints its own read tokens: drop read_token_env");
            }
            return Ok(());
        };
        if self.github_app.is_some() {
            bail!("target {q}: github_app and write_token_env exclude each other");
        }
        if self.read_token_env.as_ref() == Some(write) {
            bail!(
                "target {q}: read_token_env must not be the write token: a container never gets it",
            );
        }
        Ok(())
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        let c: Config = toml::from_str(text).context("parsing the workspace's config")?;
        if c.auto_stop_minutes < MIN_STOP_MINUTES {
            bail!("auto_stop_minutes must be at least {MIN_STOP_MINUTES}");
        }
        for d in &c.domain_allow_list {
            check_domain(d)?;
        }
        for t in &c.targets {
            t.check()?;
        }
        Ok(c)
    }

    /// Fails unless a runtime that does or does not `limits_domains` can
    /// keep what this asks.
    pub fn check_egress(&self, limits_domains: bool) -> Result<()> {
        if !self.domain_allow_list.is_empty() && !limits_domains {
            bail!("domain_allow_list is set and this runtime cannot enforce it");
        }
        let held = limits_domains && !self.domain_allow_list.is_empty();
        for t in &self.targets {
            if t.is_confidential() && !held && !t.allow_open_egress {
                bail!(
                    "target {}: a confidential target needs a domain_allow_list on a runtime \
                     that enforces it, or allow_open_egress = true, or public = true. A list \
                     narrows egress; it keeps source private only if no host on it takes \
                     uploads",
                    t.queue
                );
            }
        }
        Ok(())
    }

    /// Fails where no image can be used, a blank one counting as none, and for
    /// one that is blank on any runtime. Docker's default has no git, so a
    /// target there needs one. Daytona's default snapshot is too small to build
    /// in, and a home there needs one with no target at all: a repository a
    /// queue links runs on the file's image too. Failing here leaves the daemon
    /// up with no containers, and the panel says so.
    pub fn check_image(&self) -> Result<()> {
        let blank = self.image.as_deref().is_some_and(|i| i.trim().is_empty());
        if self.image.is_some() && !blank {
            return Ok(());
        }
        match self.runtime {
            RuntimeConfig::Daytona => bail!(
                "a home on Daytona needs an `image`, the name of a snapshot made for it, with or \
                 without a target, as a linked repository runs on it too: \
                 deploy/claxbot-workspace/README.md says how to make one"
            ),
            RuntimeConfig::Docker { .. } if !self.targets.is_empty() => {
                bail!("a target on Docker needs an `image` with git built in")
            }
            RuntimeConfig::Docker { .. } if blank => {
                bail!("`image` is blank: name one with git built in, or leave it out")
            }
            RuntimeConfig::Docker { .. } => Ok(()),
        }
    }

    /// The workspace this describes, for the daemon of `home`, taking its
    /// targets from `links` too when it is given.
    pub fn open(&self, home: &Path, links: Option<Links>) -> Result<ContainerWorkspace> {
        let runtime = self.runtime.open()?;
        self.check_egress(runtime.limits_domains())?;
        self.check_image()?;
        let labels = BTreeMap::from([(LABEL_WORKSPACE.to_string(), deployment_id(home)?)]);
        let spec = ContainerSpec {
            image: self.image.clone(),
            name: None,
            labels,
            auto_stop_minutes: Some(self.auto_stop_minutes),
            domain_allow_list: self.domain_allow_list.clone(),
        };
        let targets = self
            .targets
            .iter()
            .map(TargetConfig::open)
            .collect::<Result<_>>()?;
        let workspace = ContainerWorkspace::keeping(
            runtime,
            spec,
            targets,
            self.shell_for_all,
            home.join("workspace-runs"),
            Arc::new(Redactor::default()),
        );
        Ok(match links {
            Some(l) => {
                let egress = workspace.egress();
                workspace.with_source(Arc::new(StoreTargets::new(l.store, l.kek, egress)))
            }
            None => workspace,
        })
    }
}

impl TargetConfig {
    fn unsigned(&self) -> UnsignedCommits {
        if self.allow_unsigned_commits {
            UnsignedCommits::Allow
        } else {
            UnsignedCommits::Refuse
        }
    }

    /// Where its tokens come from. An App's key goes no further than here.
    fn credentials(&self) -> Result<Arc<dyn RepoAccess>> {
        if let Some(a) = &self.github_app {
            let key = secret(&a.private_key_env)?;
            let app = GitHubApp::new(&self.forge_api_url, a.app_id, key.expose())?;
            let access = GitHubAppAccess::new(app, a.installation_id, &self.repo, self.public)?;
            return Ok(Arc::new(access));
        }
        Ok(Arc::new(StaticAccess {
            read: self.read_token_env.as_deref().map(secret).transpose()?,
            write: self.write_token_env.as_deref().map(secret).transpose()?,
        }))
    }

    /// [`Policy::default`], and what this target says on top of it.
    fn policy(&self) -> Policy {
        let default = Policy::default();
        let mut protected_paths = default.protected_paths.clone();
        protected_paths.extend(self.protected_paths.iter().cloned());
        Policy {
            protected_paths,
            writable_paths: self.writable_paths.clone(),
            refused_in_rust: self.refused_in_rust.clone(),
            max_open_pull_requests: self
                .max_open_pull_requests
                .unwrap_or(default.max_open_pull_requests),
            max_pushes_per_day: self
                .max_pushes_per_day
                .unwrap_or(default.max_pushes_per_day),
            follow_ups: self.follow_ups,
            report_url: self.report_url.clone(),
            guidance: self.guidance.clone(),
            preview_gate: self.preview_gate.clone(),
            ..default
        }
    }

    fn open(&self) -> Result<Target> {
        let policy = self.policy();
        let access = self.credentials()?;
        let forge = GitHub::new(&self.forge_api_url, &self.repo, access.clone());
        let forge = if self.github_app.is_some() {
            forge.committing()
        } else {
            forge
        };
        Ok(Target {
            stamp: None,
            queue: self.queue.clone(),
            repo_url: self.repo_url.clone(),
            container_repo_url: self
                .container_repo_url
                .clone()
                .unwrap_or_else(|| self.repo_url.clone()),
            base: self.base.clone(),
            confidential: self.is_confidential(),
            reviewed_previews: self.reviewed_previews,
            wake_on_reply: self.wake_on_reply,
            unsigned: self.unsigned(),
            policy,
            forge: Arc::new(forge),
            access,
            preview: Preview {
                url_template: self.preview_url.clone(),
                ready_path: self.preview_ready_path.clone(),
                ready_names_commit: self.preview_names_commit,
                timeout: Duration::from_secs(self.preview_timeout_secs),
                poll: Duration::from_secs(self.preview_poll_secs),
            },
            reach: Reach::any(),
        })
    }
}

/// The workspace of the home at `home`, its links with it: `None` without a
/// `workspace.toml`. A file that is there and cannot be read is an `Err`, not
/// no file.
pub fn for_home(home: &Path, links: Option<Links>) -> Result<Option<ContainerWorkspace>> {
    let path = home.join("workspace.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    Ok(Some(Config::parse(&text)?.open(home, links)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = r#"
        [runtime]
        kind = "docker"
        [[target]]
        queue = "q"
        repo_url = "https://example.com/o/r.git"
        repo = "o/r"
        write_token_env = "W"
        allow_unsigned_commits = true
        preview_url = "https://pr-{pr}.example"
    "#;

    /// `TARGET` by a GitHub App, with `line` in the target.
    fn app_target(line: &str) -> String {
        let target = TARGET
            .replace("write_token_env = \"W\"\n", "")
            .replace("allow_unsigned_commits = true\n", "");
        format!(
            "{target}\n{line}\n[target.github_app]\napp_id = 1\ninstallation_id = 2\n\
             private_key_env = \"K\"\n"
        )
    }

    fn with(line: &str) -> String {
        TARGET.replace(
            "write_token_env = \"W\"",
            &format!("write_token_env = \"W\"\n{line}"),
        )
    }

    #[test]
    fn a_small_config_takes_the_defaults() {
        let c = Config::parse(TARGET).unwrap();
        assert_eq!(c.targets[0].base, "main");
        assert_eq!(c.targets[0].forge_api_url, "https://api.github.com");
        assert!(c.targets[0].preview_names_commit);
        assert!(!c.targets[0].reviewed_previews);
        assert!(!c.shell_for_all);
    }

    #[test]
    fn the_file_the_docs_give_for_linked_repositories_stands() {
        let c = Config::parse(
            "image = \"ghcr.io/you/build-image:latest\"\n\
             domain_allow_list = [\"github.com\", \"codeload.github.com\"]\n\
             [runtime]\nkind = \"daytona\"\n",
        )
        .unwrap();
        assert!(c.targets.is_empty());
        c.check_egress(true).unwrap();
    }

    #[test]
    fn a_runtime_is_named_only_here() {
        let c = Config::parse("[runtime]\nkind = \"daytona\"\n").unwrap();
        assert!(matches!(c.runtime, RuntimeConfig::Daytona));
        assert!(Config::parse("[runtime]\nkind = \"nope\"\n").is_err());
    }

    #[test]
    fn the_write_token_may_not_go_to_a_container() {
        let e = Config::parse(&with("read_token_env = \"W\"")).unwrap_err();
        assert!(e.to_string().contains("never gets it"));
    }

    #[test]
    fn a_github_app_stands_in_for_the_tokens() {
        let c = Config::parse(&app_target("")).unwrap();
        let app = c.targets[0].github_app.as_ref().unwrap();
        assert_eq!((app.app_id, app.installation_id), (1, 2));
        assert_eq!(app.private_key_env, "K");
    }

    #[test]
    fn a_github_app_goes_with_no_token() {
        for line in ["write_token_env = \"W\"", "read_token_env = \"R\""] {
            let e = Config::parse(&app_target(line)).unwrap_err();
            assert!(e.to_string().contains("github_app"), "{line}: {e}");
        }
    }

    #[test]
    fn a_github_app_says_all_of_itself() {
        let missing = app_target("").replace("installation_id = 2\n", "");
        assert!(Config::parse(&missing).is_err());
        let extra = app_target("").replace("app_id = 1", "app_id = 1\nprivate_key = \"x\"");
        assert!(Config::parse(&extra).is_err());
    }

    #[test]
    fn a_target_needs_a_way_to_write() {
        let none = TARGET.replace("write_token_env = \"W\"\n", "");
        let e = Config::parse(&none).unwrap_err();
        assert!(e.to_string().contains("github_app"), "{e}");
    }

    #[test]
    fn allow_unsigned_commits_says_what_a_target_carries_and_nothing_else() {
        let of = |text: &str| Config::parse(text).unwrap().targets[0].unsigned();
        assert_eq!(of(TARGET), UnsignedCommits::Allow);
        let text = TARGET.replace("allow_unsigned_commits = true\n", "");
        assert_eq!(of(&text), UnsignedCommits::Refuse);
        assert_eq!(of(&app_target("")), UnsignedCommits::Refuse);
        let allowed = app_target("allow_unsigned_commits = true");
        assert_eq!(of(&allowed), UnsignedCommits::Allow);
    }

    #[test]
    fn a_private_key_that_will_not_load_is_not_quoted() {
        // PATH is set under any test runner, and is no key.
        let text = app_target("").replace("\"K\"", "\"PATH\"");
        let c = Config::parse(&text).unwrap();
        let Err(e) = c.targets[0].open() else {
            panic!("a key that is no key loaded");
        };
        let path = std::env::var("PATH").unwrap();
        assert!(!format!("{e:#}").contains(&path), "{e:#}");
    }

    #[test]
    fn waking_on_a_reply_is_off_unless_a_target_says_so() {
        let of = |text: &str| Config::parse(text).unwrap().targets[0].wake_on_reply;
        assert!(!of(TARGET));
        assert!(of(&with("wake_on_reply = true")));
    }

    #[test]
    fn a_target_is_confidential_unless_it_says_it_is_public() {
        let of = |text: &str| Config::parse(text).unwrap().targets[0].is_confidential();
        assert!(of(TARGET));
        assert!(of(&with("read_token_env = \"R\"")));
        assert!(!of(&with("public = true")));
    }

    #[test]
    fn a_public_repository_takes_no_read_token() {
        let line = "read_token_env = \"R\"\npublic = true";
        assert!(Config::parse(&with(line)).is_err());
    }

    #[test]
    fn a_confidential_target_needs_egress_held_or_said_to_be_open() {
        let c = Config::parse(TARGET).unwrap();
        assert!(c.check_egress(false).is_err());
        assert!(c.check_egress(true).is_err(), "no list to hold it to");

        let listed = format!("domain_allow_list = [\"a.dev\"]\n{TARGET}");
        let c = Config::parse(&listed).unwrap();
        assert!(c.check_egress(true).is_ok());

        let c = Config::parse(&with("allow_open_egress = true")).unwrap();
        assert!(c.check_egress(false).is_ok());
        let c = Config::parse(&with("public = true")).unwrap();
        assert!(c.check_egress(false).is_ok());
    }

    #[test]
    fn an_allow_list_a_runtime_cannot_hold_fails_at_startup() {
        let text = format!("domain_allow_list = [\"a.dev\"]\n{}", with("public = true"));
        let e = Config::parse(&text)
            .unwrap()
            .check_egress(false)
            .unwrap_err();
        assert!(e.to_string().contains("cannot enforce"));
    }

    #[test]
    fn a_url_holding_credentials_or_sending_a_token_in_the_clear_is_refused() {
        let swap = |url: &str| TARGET.replace("https://example.com/o/r.git", url);
        assert!(Config::parse(&swap("https://u:p@example.com/r.git")).is_err());
        assert!(Config::parse(&swap("https://tok@example.com/r.git")).is_err());
        assert!(Config::parse(&swap("http://example.com/r.git")).is_err());
        assert!(Config::parse(&swap("http://localhost:9/r.git")).is_ok());
        assert!(Config::parse(&swap("/srv/repo.git")).is_ok());
    }

    #[test]
    fn ssh_urls_are_refused_in_every_spelling() {
        let swap = |url: &str| TARGET.replace("https://example.com/o/r.git", url);
        for url in [
            "ssh://git@github.com/o/r.git",
            "git@github.com:o/r.git",
            "tok@host:path",
            "host:path",
        ] {
            let e = Config::parse(&swap(url)).unwrap_err();
            assert!(e.to_string().contains("ssh"), "{url}: {e}");
        }
    }

    #[test]
    fn an_allow_list_entry_must_name_a_host() {
        for entry in ["*", "", "*.*", " ", "a.dev,b.dev"] {
            let text = format!("domain_allow_list = [{entry:?}]\n{TARGET}");
            assert!(Config::parse(&text).is_err(), "{entry:?}");
        }
        let text = format!("domain_allow_list = [\"*.daytona.io\"]\n{TARGET}");
        assert!(Config::parse(&text).is_ok());
    }

    #[test]
    fn a_target_on_docker_needs_an_image() {
        assert!(Config::parse(TARGET).unwrap().check_image().is_err());
        let text = format!("image = \"mine\"\n{TARGET}");
        assert!(Config::parse(&text).unwrap().check_image().is_ok());
        let shell = "shell_for_all = true\n[runtime]\nkind = \"docker\"\n";
        assert!(Config::parse(shell).unwrap().check_image().is_ok());
    }

    #[test]
    fn a_target_on_daytona_needs_a_snapshot() {
        let daytona = TARGET.replace("kind = \"docker\"", "kind = \"daytona\"");
        let why = Config::parse(&daytona).unwrap().check_image().unwrap_err();
        assert!(why.to_string().contains("snapshot"), "{why}");
        let text = format!("image = \"mine\"\n{daytona}");
        assert!(Config::parse(&text).unwrap().check_image().is_ok());
    }

    /// The linked route's file, as the README has it: its repositories run on
    /// this image, so it is no less needed with no target.
    #[test]
    fn a_home_on_daytona_needs_a_snapshot_with_no_target() {
        for text in [
            "[runtime]\nkind = \"daytona\"\n",
            "shell_for_all = true\n[runtime]\nkind = \"daytona\"\n",
        ] {
            let why = Config::parse(text).unwrap().check_image().unwrap_err();
            assert!(why.to_string().contains("snapshot"), "{why}");
            let named = format!("image = \"mine\"\n{text}");
            assert!(Config::parse(&named).unwrap().check_image().is_ok());
        }
    }

    #[test]
    fn a_blank_image_is_no_image() {
        let daytona = TARGET.replace("kind = \"docker\"", "kind = \"daytona\"");
        let bare = "[runtime]\nkind = \"docker\"\n";
        let linked = "[runtime]\nkind = \"daytona\"\n";
        for blank in ["", "  "] {
            for text in [TARGET, daytona.as_str(), bare, linked] {
                let text = format!("image = {blank:?}\n{text}");
                let c = Config::parse(&text).unwrap();
                assert!(c.check_image().is_err(), "{text}");
            }
        }
    }

    #[test]
    fn a_target_whose_previews_a_person_approves_says_what_they_do() {
        let e = Config::parse(&app_target("reviewed_previews = true")).unwrap_err();
        let why = e.to_string();
        assert!(why.contains("preview_gate"), "{why}");
        assert!(why.contains("/preview {commit}"), "{why}");
        for blank in ["", "  "] {
            let text = app_target(&format!(
                "reviewed_previews = true\npreview_gate = {blank:?}"
            ));
            let e = Config::parse(&text).unwrap_err();
            assert!(e.to_string().contains("preview_gate"), "{e}");
        }
        let gated = app_target("reviewed_previews = true\npreview_gate = \"press go\"");
        assert!(Config::parse(&gated).unwrap().targets[0].reviewed_previews);
        // A gate with no review is a target's own business.
        assert!(Config::parse(&app_target("preview_gate = \"press go\"")).is_ok());
    }

    #[test]
    fn the_clax_profile_waits_a_day_for_a_person_and_a_deploy() {
        let text = include_str!("../../../deploy/claxbot-workspace/clax.toml");
        let target = &Config::parse(text).unwrap().targets[0];
        assert_eq!(target.preview_timeout_secs, 86_400);
        assert_eq!(target.preview_poll_secs, 60);
        let url = target.report_url.as_deref().unwrap();
        assert_eq!(
            url,
            "https://clax.dev/queues/REPLACE_WITH_QUEUE_ID/{report}"
        );
    }

    #[test]
    fn auto_stop_must_stop() {
        for minutes in [0, 14] {
            let text = format!("auto_stop_minutes = {minutes}\n{TARGET}");
            assert!(Config::parse(&text).is_err());
        }
        let text = format!("auto_stop_minutes = 15\n{TARGET}");
        assert!(Config::parse(&text).is_ok());
    }

    #[test]
    fn a_home_keeps_its_own_deployment_id() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let first = deployment_id(a.path()).unwrap();
        assert_eq!(deployment_id(a.path()).unwrap(), first);
        std::fs::write(b.path().join("workspace-id"), "another").unwrap();
        assert_ne!(deployment_id(b.path()).unwrap(), first);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_no_absent_file() {
        let home = tempfile::tempdir().unwrap();
        assert!(for_home(home.path(), None).unwrap().is_none());
        // A directory by that name reads as an error, as a file nobody may
        // read does.
        std::fs::create_dir(home.path().join("workspace.toml")).unwrap();
        assert!(for_home(home.path(), None).is_err());
    }

    /// `path-cases.txt`, which `scripts/check-claxbot-paths.py` reads as well:
    /// each line `allow` or `refuse`, then a path.
    fn path_cases() -> Vec<(bool, &'static str)> {
        include_str!("../../../deploy/claxbot-workspace/path-cases.txt")
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let (word, path) = l.split_once(' ').unwrap();
                assert!(matches!(word, "allow" | "refuse"), "{l}");
                (word == "allow", path)
            })
            .collect()
    }

    fn clax_policy() -> Policy {
        let text = include_str!("../../../deploy/claxbot-workspace/clax.toml");
        Config::parse(text).unwrap().targets[0].policy()
    }

    #[test]
    fn the_clax_profile_judges_every_listed_path_as_the_list_says() {
        let policy = clax_policy();
        let cases = path_cases();
        assert!(cases.len() > 20, "{}", cases.len());
        for (allowed, path) in cases {
            let may = policy.may_write(path) && !policy.protects(path);
            assert_eq!(may, allowed, "{path}");
        }
    }

    #[test]
    fn the_clax_profile_refuses_a_manifest_a_build_script_and_a_lockfile() {
        let policy = clax_policy();
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "crates/clax-core/Cargo.toml",
            "crates/clax-core/build.rs",
            "crates/clax-core/src/build.rs",
            "crates/clax-core/src/Cargo.toml",
            "web/package.json",
            "web/pnpm-lock.yaml",
            "web/shell/src/package.json",
            "web/e2e/.npmrc",
        ] {
            let edit = format!(":100644 100644 aaa bbb M\0{path}\0");
            assert!(
                crate::repo::refusal(edit.as_bytes(), &policy).is_some(),
                "{path}"
            );
        }
    }

    #[test]
    fn the_clax_profile_lists_the_defaults_for_the_check_that_reads_it_alone() {
        let policy = clax_policy();
        let listed = &Config::parse(include_str!("../../../deploy/claxbot-workspace/clax.toml"))
            .unwrap()
            .targets[0]
            .protected_paths;
        for default in Policy::default().protected_paths {
            assert!(listed.contains(&default), "{default}");
        }
        assert!(!policy.writable_paths.is_empty());
    }

    #[test]
    fn the_clax_profile_is_held_tight() {
        let policy = clax_policy();
        assert!(policy.refused_in_rust.contains(&"#[path".to_string()));
        assert_eq!(policy.max_open_pull_requests, 3);
        assert_eq!(policy.max_pushes_per_day, 10);
        assert!(!policy.follow_ups);
    }

    #[test]
    fn a_target_that_says_nothing_of_limits_takes_the_generous_defaults() {
        let policy = Config::parse(TARGET).unwrap().targets[0].policy();
        let default = Policy::default();
        assert_eq!(policy.max_pushes_per_day, default.max_pushes_per_day);
        assert_eq!(
            policy.max_open_pull_requests,
            default.max_open_pull_requests
        );
        assert!(policy.follow_ups && policy.writable_paths.is_empty());
        assert!(default.max_open_pull_requests >= 20 && default.max_pushes_per_day >= 100);
    }

    #[test]
    fn a_target_may_say_its_own_limits() {
        let text = with("max_open_pull_requests = 2\nmax_pushes_per_day = 4\nfollow_ups = false");
        let policy = Config::parse(&text).unwrap().targets[0].policy();
        assert_eq!(policy.max_open_pull_requests, 2);
        assert_eq!(policy.max_pushes_per_day, 4);
        assert!(!policy.follow_ups);
    }

    #[test]
    fn a_path_pattern_that_cannot_mean_what_it_says_fails_at_startup() {
        for key in ["protected_paths", "writable_paths"] {
            let e = Config::parse(&with(&format!("{key} = [\"a/**/b\"]"))).unwrap_err();
            assert!(e.to_string().contains("a/**/b"), "{e}");
        }
    }

    #[test]
    fn a_report_address_must_name_the_report_and_go_over_https() {
        let of = |url: &str| Config::parse(&with(&format!("report_url = {url:?}")));
        assert!(of("https://clax.dev/r/{report}").is_ok());
        assert!(of("https://clax.dev/r/").is_err());
        assert!(of("http://clax.dev/r/{report}").is_err());
    }

    #[test]
    fn a_targets_words_for_its_runs_have_limits() {
        let of =
            |field: &str, n: usize| Config::parse(&with(&format!("{field} = {:?}", "x".repeat(n))));
        let c = of("guidance", MAX_GUIDANCE_BYTES).unwrap();
        assert_eq!(
            c.targets[0].policy().guidance.map(|g| g.len()),
            Some(MAX_GUIDANCE_BYTES)
        );
        let e = of("guidance", MAX_GUIDANCE_BYTES + 1).unwrap_err();
        assert!(e.to_string().contains("guidance"), "{e}");
        let c = of("preview_gate", MAX_PREVIEW_GATE_BYTES).unwrap();
        assert!(c.targets[0].policy().preview_gate.is_some());
        let e = of("preview_gate", MAX_PREVIEW_GATE_BYTES + 1).unwrap_err();
        assert!(e.to_string().contains("preview_gate"), "{e}");
        assert!(
            Config::parse(&with("")).unwrap().targets[0]
                .guidance
                .is_none()
        );
    }

    #[test]
    fn clax_says_how_to_work_in_it_and_what_deploys_a_preview() {
        let text = include_str!("../../../deploy/claxbot-workspace/clax.toml");
        let target = Config::parse(text).unwrap().targets.remove(0);
        let guidance = target.guidance.unwrap();
        for words in [
            "`just`",
            "not cargo",
            "`timeout: 600`",
            "bazelisk test //crates/<crate>:...",
            "`type(scope): summary`",
            "attribution",
            "manifests, lockfiles or build scripts",
            "sign-in, authorization and report queues",
            "no qualifier records",
            "web/perf/bundle-budget.json",
            "test first",
        ] {
            assert!(guidance.contains(words), "{words} in {guidance}");
        }
        assert!(!guidance.contains(".qual"), "{guidance}");
        assert_eq!(
            target.preview_gate.as_deref(),
            Some(
                "read the change, then, with write access, post `/preview {commit}` on the pull request"
            )
        );
    }

    #[test]
    fn the_example_profiles_parse() {
        let profile = |text: &str| Config::parse(text).unwrap().targets.remove(0);
        let clax = include_str!("../../../deploy/claxbot-workspace/clax.toml");
        let changelist = include_str!("../../../deploy/claxbot-workspace/changelist.toml");
        let pathbase = include_str!("../../../deploy/claxbot-workspace/pathbase.toml");
        assert!(profile(clax).preview_names_commit);
        assert!(!profile(changelist).preview_names_commit);
        assert!(!profile(pathbase).preview_names_commit);
        assert!(profile(clax).public && profile(clax).reviewed_previews);
        for text in [clax, changelist, pathbase] {
            Config::parse(text).unwrap().check_egress(true).unwrap();
            assert!(profile(text).github_app.is_some());
        }
    }
}

//! [`clax_bot::Workspace`] over a [`ContainerRuntime`]: one container per
//! run, made at its first call. A run on a queue with a [`Target`] also gets
//! that repository, and may propose a change to it.
//!
//! A daemon that restarts forgets its runs, and a resumed run's container
//! still holds its edits. So the [`Ledger`] keeps each run's container and
//! labels ([`LABEL_RUN`], [`LABEL_QUEUE`], [`LABEL_BASE`]), a run with no
//! container in memory takes its own back before making one, and the host's
//! clone is rebuilt from them. When a run had a container and it is gone, the
//! run is told it was lost; it never silently starts over. A runtime that
//! could not be asked is not a loss: the next call asks again.
//!
//! Targets come from a deployment's file or, per run, from a [`TargetSource`]:
//! the file's wins. A run holds the target it started on, credentials and all,
//! until it ends, and keeps what it was offered ([`Entry::offer`]) whatever
//! becomes of the target, but for one thing: a target that says its material
//! is confidential says so of the run too. A run that has not begun takes the
//! target as it is. Only what is in memory holds credentials: a run that lost
//! them, to a restart or a container that stopped answering, and finds its
//! target gone or standing on another repository ([`Entry::repository`]) keeps
//! its shell, and can publish nothing more. What became of a source's
//! credentials goes back to it, in fixed words.
//!
//! A thread's work stands on `claxbot/<thread>`. A pull request merged or
//! closed ends its branch: a later run goes on only from a branch with an open
//! pull request, and otherwise starts from the target's current base on the
//! next of `claxbot/<thread>-N`.
//!
//! A run reads neutral words about a failure. The detail, which can name the
//! runtime or quote a remote, goes to the daemon's log, redacted.
//!
//! `propose` does not wait for a preview. It answers with `verify_url` null
//! and a detached task, which holds no lock of the run's and may outlive it,
//! tells the [`PreviewSink`] when the preview is up, or that the wait ended
//! with none seen. The [`Ledger`] keeps a wait until its time is up, a finished run's
//! too, for a restart to take up; a proposal answered again from the ledger
//! takes it up as well, since what was told before its fix was on the report
//! went nowhere.
//!
//! A target says how much it takes ([`Policy`]): what a change may touch, and
//! how many open pull requests and pushes a day. Each is checked before
//! anything leaves, in fixed words, against the forge's count and the
//! [`Ledger`]'s, so a restart spends nothing back. A pull request's text is the
//! host's first and the model's after, quoted. Every proposal writes the host's
//! part again, so what it lists is what the branch holds, and leaves what follows
//! as a person left it.
//!
//! A forge that [commits](Forge::commits) moves the branch without the host's
//! clone seeing, so each proposal starts by fetching it. That is what lets a
//! later proposal continue from the forge's commit, tells a lost answer from
//! a failure, and finds a commit of anyone else's.

use crate::forge::{
    Forge, NewCommit, NewPullRequest, PullRequest, Refused, UnsignedCommits, commit_subject,
    defuse_mentions, host_block, pull_request_body, strip_html_comments, with_host_block,
};
use crate::ledger::{Awaiting, Entry, Ledger, Pending, Repository};
use crate::linked::{Described, TargetSource};
use crate::net::Reach;
use crate::preview::Preview;
use crate::repo::{self, AccessRefused, Policy, PushRefused, Redactor, Remembered, RepoAccess};
use crate::runtime::{
    Container, ContainerRuntime, ContainerSpec, Exec, Found, MAX_EXEC_OUTPUT, is_gone,
};
use anyhow::{Context, Result, bail};
use base64::Engine;
use clax_bot::workspace::{
    CODE_CHANGE_REFUSED, CODE_SHELL_UNAVAILABLE, Offer, PreviewSink, Proposal, Workspace,
};
use clax_bot_sandbox::HostError;
use clax_store::bot_runs::Run;
use clax_store::queue_repos::{Egress, PUBLISH_FAILED, RepoUse};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::AbortHandle;

/// What every branch of ClaxBot's starts with.
const BRANCH_PREFIX: &str = "claxbot/";
/// Where the repository is in a container, as `$CLAX_REPO`.
pub const REPO_DIR: &str = "/tmp/clax-repo";
/// The run a container belongs to.
pub const LABEL_RUN: &str = "clax.run";
/// The target's queue, on a container with a checkout.
pub const LABEL_QUEUE: &str = "clax.queue";
/// The commit its checkout started from, on a container with a checkout.
pub const LABEL_BASE: &str = "clax.base";
/// The deployment a container belongs to, in `ContainerWorkspace::spec`.
pub const LABEL_WORKSPACE: &str = "clax.workspace";
const SETUP_TIMEOUT: Duration = Duration::from_secs(600);
const PATCH_TIMEOUT: Duration = Duration::from_secs(120);
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// What a link's people read when a use of its credentials failed.
const OPEN_FAILED: &str = "The daemon could not open this link's credentials: set them again.";
const HOST_CLONE_FAILED: &str = "The daemon could not clone the repository with these credentials.";
const CONTAINER_CLONE_FAILED: &str = "The container could not clone the repository.";

/// A repository a queue's runs work on.
pub struct Target {
    pub queue: String,
    /// Which credentials a [`TargetSource`]'s target holds, for what it is
    /// told of them; `None` for an operator's.
    pub stamp: Option<String>,
    /// Where the host clones and pushes.
    pub repo_url: String,
    /// Where a container clones from.
    pub container_repo_url: String,
    pub base: String,
    /// The tokens: asked each time, for the host's write token and the
    /// container's one clone. The workspace notes all that this issues, to
    /// keep them out of what it says.
    pub access: Arc<dyn RepoAccess>,
    /// That each deploy of it needs a person's approval of the commit.
    pub reviewed_previews: bool,
    /// That a reporter's plain reply to a waiting fix starts a run.
    pub wake_on_reply: bool,
    /// Whether a commit nobody signed may be published.
    pub unsigned: UnsignedCommits,
    /// What a container holds of it is not for a report's reporter.
    pub confidential: bool,
    pub policy: Policy,
    pub forge: Arc<dyn Forge>,
    pub preview: Preview,
    pub reach: Reach,
}

impl Target {
    /// Where a change of this target's goes.
    fn repository(&self) -> Repository {
        Repository {
            forge: self.forge.repository(),
            repo_url: self.repo_url.clone(),
            container_repo_url: self.container_repo_url.clone(),
        }
    }
}

/// What a run's container has been given.
struct Checkout {
    target: Arc<Target>,
    branch: String,
    /// The commit the container's checkout started from, which its change
    /// is always against.
    base: String,
    clone: repo::Clone,
    /// Holds the host's clone for as long as the run.
    _dir: tempfile::TempDir,
}

/// The host's clone, ready before a container is asked for.
struct HostClone {
    clone: repo::Clone,
    dir: tempfile::TempDir,
}

struct RunBox {
    container: Box<dyn Container>,
    checkout: Option<Checkout>,
    /// The branch of a repository this container holds with no target left
    /// to publish it through: a restart found the target gone.
    orphan: Option<String>,
}

/// The waits under way, by run: a number to tell one from its successor.
type Waits = Arc<Mutex<HashMap<String, (u64, AbortHandle)>>>;

/// A deployment's workspace.
pub struct ContainerWorkspace {
    pub runtime: Arc<dyn ContainerRuntime>,
    /// Its `labels` tell this deployment's containers from another's on the
    /// same runtime, and must be its own for [`Self::end_finished`] to be safe.
    pub spec: ContainerSpec,
    targets: Vec<Arc<Target>>,
    /// Targets for queues `targets` does not name.
    source: Option<Arc<dyn TargetSource>>,
    /// Whether a run with no target gets a bare container.
    pub shell_for_all: bool,
    ledger: Ledger,
    secrets: Arc<Redactor>,
    sink: Mutex<Option<Arc<dyn PreviewSink>>>,
    waits: Waits,
    wait_number: AtomicU64,
    runs: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<RunBox>>>>>,
    /// Held from a proposal's count of pull requests to its opening one, so
    /// that two runs cannot both take the last.
    proposing: tokio::sync::Mutex<()>,
}

/// `t`, noting in `secrets` all its access issues.
fn remembered(t: Target, secrets: &Arc<Redactor>) -> Target {
    Target {
        access: Arc::new(Remembered {
            inner: t.access,
            redactor: secrets.clone(),
        }),
        ..t
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// What tells a run its container will not come back. The detail stays in
/// the daemon's log.
fn lost(run: &Run, why: &str) -> HostError {
    tracing::warn!(run = run.id, why, "a run's workspace was lost");
    HostError::new(
        CODE_SHELL_UNAVAILABLE,
        "this run's workspace was lost: its container, and every edit made in it, \
         cannot be recovered, and no new one was made. Report that instead of going on.",
    )
}

/// The shell text that puts the repository at `url` in [`REPO_DIR`], at
/// commit `base`.
pub fn clone_command(url: &str, base: &str) -> String {
    format!(
        "git clone --quiet --no-hardlinks -- {} {REPO_DIR} && git -C {REPO_DIR} checkout --quiet --detach {base}",
        sh_quote(url)
    )
}

/// The shell text that answers the container's change against `base` as
/// base64. Only base64 reaches stdout, since a runtime merges stderr into the
/// output, and a failure in any part of it fails the whole.
fn patch_command(base: &str) -> String {
    format!(
        "f=$(mktemp) && trap 'rm -f \"$f\" \"$f.err\"' EXIT && \
         {{ git add -A >/dev/null && git diff --cached --binary {base} >\"$f\"; }} \
         2>\"$f.err\" && base64 <\"$f\" || {{ cat \"$f.err\"; exit 1; }}"
    )
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn is_commit_id(s: &str) -> bool {
    s.len() >= 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What a run on a target that says these things gets.
fn change_offer(
    confidential: bool,
    reviewed_previews: bool,
    wake_on_reply: bool,
    follow_ups: bool,
    guidance: Option<String>,
    preview_gate: Option<String>,
) -> Offer {
    Offer {
        confidential,
        reviewed_previews,
        wake_on_reply,
        no_follow_ups: !follow_ups,
        guidance,
        preview_gate,
        ..Offer::CHANGE
    }
}

/// Whether what `entry`'s run was offered lets a preview out to a reporter.
fn reviewed_of(entry: &Entry) -> bool {
    entry.offer.as_ref().is_some_and(|o| o.reviewed_previews)
}

/// The branch a thread's first work stands on.
fn thread_branch(thread: &str) -> String {
    format!("{BRANCH_PREFIX}{thread}")
}

/// What a pull request calls the report it came from: a link where `policy`
/// gives an address, and otherwise its ID.
fn report_name(policy: &Policy, thread: &str) -> String {
    match &policy.report_url {
        Some(url) => format!("[`{thread}`]({})", url.replace("{report}", thread)),
        None => format!("`{thread}`"),
    }
}

/// A proposal's answer. `verify_url` is null: the sink hears of a preview.
fn proposed(pr: &PullRequest, commit: &str) -> Value {
    json!({"pr": pr.number, "url": pr.html_url, "commit": commit, "verify_url": null})
}

impl ContainerWorkspace {
    /// A workspace whose ledger goes with it, so that it recovers nothing
    /// across a restart.
    pub fn new(
        runtime: Arc<dyn ContainerRuntime>,
        spec: ContainerSpec,
        targets: Vec<Target>,
        shell_for_all: bool,
    ) -> ContainerWorkspace {
        let ledger = Ledger::temporary().expect("the temporary directory is writable");
        let secrets = Arc::default();
        Self::build(runtime, spec, targets, shell_for_all, ledger, secrets)
    }

    /// A workspace keeping what it knows of runs in `dir`, which outlives
    /// the daemon, and taking tokens out of text by `secrets`.
    pub fn keeping(
        runtime: Arc<dyn ContainerRuntime>,
        spec: ContainerSpec,
        targets: Vec<Target>,
        shell_for_all: bool,
        dir: PathBuf,
        secrets: Arc<Redactor>,
    ) -> ContainerWorkspace {
        let ledger = Ledger::at(dir);
        Self::build(runtime, spec, targets, shell_for_all, ledger, secrets)
    }

    fn build(
        runtime: Arc<dyn ContainerRuntime>,
        spec: ContainerSpec,
        targets: Vec<Target>,
        shell_for_all: bool,
        ledger: Ledger,
        secrets: Arc<Redactor>,
    ) -> ContainerWorkspace {
        let targets = targets
            .into_iter()
            .map(|t| Arc::new(remembered(t, &secrets)))
            .collect();
        ContainerWorkspace {
            runtime,
            spec,
            targets,
            source: None,
            shell_for_all,
            ledger,
            secrets,
            sink: Mutex::new(None),
            waits: Arc::default(),
            wait_number: AtomicU64::new(0),
            runs: Mutex::new(HashMap::new()),
            proposing: tokio::sync::Mutex::new(()),
        }
    }

    /// This workspace, taking the queues its file does not name from
    /// `source`.
    #[must_use]
    pub fn with_source(self, source: Arc<dyn TargetSource>) -> ContainerWorkspace {
        ContainerWorkspace {
            source: Some(source),
            ..self
        }
    }

    /// Whether this workspace holds its containers to an allow-list.
    pub fn egress(&self) -> Egress {
        if self.runtime.limits_domains() && !self.spec.domain_allow_list.is_empty() {
            Egress::Held
        } else {
            Egress::Open
        }
    }

    /// The queues whose target an operator wrote.
    pub fn operator_queues(&self) -> Vec<String> {
        self.targets.iter().map(|t| t.queue.clone()).collect()
    }

    /// The runs [`Workspace::offer`] answers from what they keep, each with
    /// its target's queue. A run whose entry cannot be read is among them:
    /// `offer` answers it [`Offer::unsure`].
    pub fn runs_keeping_offers(&self) -> Vec<(String, Option<String>)> {
        let kept = |id: String| match self.ledger.read(&id) {
            Ok(e) if e.finished || e.offer.is_none() => None,
            Ok(e) => {
                let queue = e.labels.get(LABEL_QUEUE).cloned();
                Some((id, queue))
            }
            Err(_) => Some((id, None)),
        };
        self.ledger.runs().into_iter().filter_map(kept).collect()
    }

    /// When the last wait for a preview ends, a finished run's too, in
    /// seconds since the epoch.
    pub fn previews_until(&self) -> Option<u64> {
        let until = |id: String| self.ledger.read(&id).ok()?.awaiting.map(|a| a.until);
        self.ledger.runs().into_iter().filter_map(until).max()
    }

    fn file_target(&self, queue: &str) -> Option<&Arc<Target>> {
        self.targets.iter().find(|t| t.queue == queue)
    }

    /// `queue`'s target as of now, opening nothing. An `Err` says nothing of
    /// it, and is the caller's to answer as for confidential material.
    fn described(&self, queue: Option<&str>) -> Result<Option<Described>> {
        let Some(q) = queue else {
            return Ok(None);
        };
        if let Some(t) = self.file_target(q) {
            return Ok(Some(Described {
                confidential: t.confidential,
                reviewed_previews: t.reviewed_previews,
                wake_on_reply: t.wake_on_reply,
                follow_ups: t.policy.follow_ups,
                guidance: t.policy.guidance.clone(),
                preview_gate: t.policy.preview_gate.clone(),
                preview: t.preview.clone(),
                preview_http: t.reach.preview.clone(),
            }));
        }
        self.source.as_ref().map_or(Ok(None), |s| s.describe(q))
    }

    /// Whether `queue`'s target says its material is confidential as of now,
    /// usable or not. Not knowing is taken as yes: this is asked of a run that
    /// has begun, and what it holds is not for a reporter until told so.
    fn confidential_now(&self, run: &Run, queue: Option<&str>) -> bool {
        let Some(q) = queue else {
            return false;
        };
        if let Some(t) = self.file_target(q) {
            return t.confidential;
        }
        let Some(source) = &self.source else {
            return false;
        };
        source.confidential(q).unwrap_or_else(|e| {
            let detail = self.safe(&format!("{e:#}"));
            tracing::warn!(run = run.id, detail, "a run's target could not be read");
            true
        })
    }

    /// `queue`'s target, its credentials opened for one run.
    async fn resolved(&self, queue: Option<&str>) -> Result<Option<Arc<Target>>> {
        let Some(q) = queue else {
            return Ok(None);
        };
        if let Some(t) = self.file_target(q) {
            return Ok(Some(t.clone()));
        }
        let Some(source) = &self.source else {
            return Ok(None);
        };
        let opened = source
            .open(q)
            .await
            .inspect_err(|_| source.failed(q, None, OPEN_FAILED))?;
        Ok(opened.map(|t| Arc::new(remembered(t, &self.secrets))))
    }

    /// The source of `t`, unless a file names it: what became of its
    /// credentials is told there.
    fn source_of(&self, t: &Target) -> Option<&Arc<dyn TargetSource>> {
        self.source
            .as_ref()
            .filter(|_| self.file_target(&t.queue).is_none())
    }

    fn worked(&self, t: &Target, used: RepoUse) {
        if let (Some(s), Some(stamp)) = (self.source_of(t), &t.stamp) {
            s.succeeded(&t.queue, stamp, used);
        }
    }

    fn failed(&self, t: &Target, what: &str) {
        if let Some(s) = self.source_of(t) {
            s.failed(&t.queue, t.stamp.as_deref(), what);
        }
    }

    fn slot(&self, run: &Run) -> Arc<tokio::sync::Mutex<Option<RunBox>>> {
        let mut runs = self.runs.lock().expect("the runs' lock");
        runs.entry(run.id.clone()).or_default().clone()
    }

    fn safe(&self, text: &str) -> String {
        self.secrets.clean(text)
    }

    /// A failure as a run sees it, its detail logged. The same call may
    /// work if made again.
    fn unavailable(&self, run: &Run, e: &anyhow::Error) -> HostError {
        let detail = self.safe(&format!("{e:#}"));
        tracing::warn!(run = run.id, detail, "a run's container is out of reach");
        if e.downcast_ref::<AccessRefused>().is_some() {
            return HostError::new(
                CODE_SHELL_UNAVAILABLE,
                "the repository refused its credentials and nothing was changed. Asking again \
                 will not help: report that a person must set them again.",
            );
        }
        HostError::new(
            CODE_SHELL_UNAVAILABLE,
            "the container could not be reached; nothing was changed. Try again.",
        )
        .retryable()
    }

    /// A failure of the forge's on a commit. What it refused will not go
    /// better for asking again, and the run is told so.
    fn by_forge(&self, run: &Run, t: &Target, e: &anyhow::Error) -> HostError {
        if e.downcast_ref::<Refused>().is_none() {
            return self.unavailable(run, e);
        }
        self.failed(t, PUBLISH_FAILED);
        let detail = self.safe(&format!("{e:#}"));
        tracing::warn!(run = run.id, detail, "the forge refused a change");
        HostError::new(
            CODE_CHANGE_REFUSED,
            "the forge refused this change and nothing was published. Report that instead of \
             proposing it again.",
        )
    }

    /// A failure of a `git push`. What the remote would not take will not go
    /// better for asking again, and no credential is at fault for it.
    fn by_push(&self, run: &Run, e: &anyhow::Error) -> HostError {
        if e.downcast_ref::<PushRefused>().is_none() {
            return self.unavailable(run, e);
        }
        let detail = self.safe(&format!("{e:#}"));
        tracing::warn!(run = run.id, detail, "a remote refused a push");
        HostError::new(
            CODE_CHANGE_REFUSED,
            "the repository would not take this push, as its branch moved or a rule of its own \
             stopped it, so nothing was published. Report that instead of proposing it again.",
        )
    }

    /// A failure once the change has left: what a run reads must not say
    /// nothing changed.
    fn after_push(&self, run: &Run, e: &anyhow::Error) -> HostError {
        let detail = self.safe(&format!("{e:#}"));
        tracing::warn!(run = run.id, detail, "a pushed change has no pull request");
        HostError::new(
            CODE_SHELL_UNAVAILABLE,
            "the change was pushed, but its pull request could not be opened. Propose it again \
             to finish.",
        )
        .retryable()
    }

    /// Run `run`'s ledger entry, refused once the run has finished. Read
    /// with the run's slot held, or [`Workspace::finish`] can pass between.
    fn entry(&self, run: &Run) -> Result<Entry, HostError> {
        let entry = self
            .ledger
            .read(&run.id)
            .map_err(|e| self.unavailable(run, &e))?;
        if entry.finished {
            return Err(HostError::new(CODE_SHELL_UNAVAILABLE, "this run has ended"));
        }
        Ok(entry)
    }

    fn labels_of(&self, run: &Run) -> BTreeMap<String, String> {
        let mut labels = self.spec.labels.clone();
        labels.insert(LABEL_RUN.into(), run.id.clone());
        labels
    }

    async fn host_clone(&self, t: &Target) -> Result<HostClone> {
        let dir = tempfile::tempdir().context("making a scratch directory")?;
        let token = t.access.write_token().await?;
        let path = dir.path().join("repo");
        let rule = t.reach.git.clone();
        let clone = repo::Clone::reaching(&t.repo_url, &path, token.as_ref(), rule).await;
        match clone {
            Ok(clone) => {
                self.worked(t, RepoUse::Read);
                Ok(HostClone { clone, dir })
            }
            Err(e) => {
                self.failed(t, HOST_CLONE_FAILED);
                Err(e)
            }
        }
    }

    /// Makes the run's container, with its checkout when it has a target.
    /// The host's clone comes first: the commit it stands on goes on the
    /// container as a label, and a label is set at creation.
    async fn start(&self, run: &Run, queue: Option<&str>) -> Result<RunBox> {
        let mut spec = self.spec.clone();
        spec.name = Some(format!("clax-{}", run.id.to_lowercase()));
        spec.labels.insert(LABEL_RUN.into(), run.id.clone());
        let Some(target) = self.resolved(queue).await? else {
            let container = self.runtime.create(&spec).await?;
            self.remember(run, container.as_ref(), &spec, Offer::SHELL, None, None)?;
            return Ok(RunBox {
                container,
                checkout: None,
                orphan: None,
            });
        };
        let t = target.as_ref();
        let thread = run.thread_ids.first().context("the run has no thread")?;
        let host = self.host_clone(t).await?;
        let branch = Self::branch_for(t, &host.clone, thread).await?;
        let base = host.clone.start_branch(&branch, &t.base).await?;
        spec.labels.insert(LABEL_QUEUE.into(), t.queue.clone());
        spec.labels.insert(LABEL_BASE.into(), base.clone());
        let container = self.runtime.create(&spec).await?;
        let offer = change_offer(
            t.confidential,
            t.reviewed_previews,
            t.wake_on_reply,
            t.policy.follow_ups,
            t.policy.guidance.clone(),
            t.policy.preview_gate.clone(),
        );
        let cloned = async {
            let repository = Some(t.repository());
            self.remember(
                run,
                container.as_ref(),
                &spec,
                offer,
                Some(&branch),
                repository,
            )?;
            self.check_out(container.as_ref(), t, &base).await?;
            let entry = Entry {
                checked_out: true,
                ..self.ledger.read(&run.id)?
            };
            self.ledger.write(&run.id, &entry)
        };
        if let Err(e) = cloned.await {
            self.failed(t, CONTAINER_CLONE_FAILED);
            if let Err(d) = self.runtime.destroy(container.id()).await {
                tracing::warn!(error = %d, "destroying a container failed");
            }
            self.ledger.remove(&run.id);
            return Err(e);
        }
        Ok(RunBox {
            container,
            checkout: Some(Checkout {
                target,
                branch,
                base,
                clone: host.clone,
                _dir: host.dir,
            }),
            orphan: None,
        })
    }

    /// The branch a new run on `thread` works on: the newest of the thread's
    /// with an open pull request, or a name no branch has, which starts from
    /// the base. Any other of the thread's branches is finished.
    async fn branch_for(t: &Target, clone: &repo::Clone, thread: &str) -> Result<String> {
        let name = thread_branch(thread);
        let taken = clone.numbered_branches(&name).await?;
        for (_, branch) in &taken {
            if t.forge.find_open(branch).await?.is_some() {
                return Ok(branch.clone());
            }
        }
        Ok(match taken.first() {
            Some((n, _)) => format!("{name}-{}", n + 1),
            None => name,
        })
    }

    fn remember(
        &self,
        run: &Run,
        c: &dyn Container,
        spec: &ContainerSpec,
        offer: Offer,
        branch: Option<&str>,
        repository: Option<Repository>,
    ) -> Result<()> {
        let entry = Entry {
            container: Some(c.id().to_string()),
            labels: spec.labels.clone(),
            offer: Some(offer),
            branch: branch.map(str::to_string),
            repository,
            ..self.ledger.read(&run.id)?
        };
        self.ledger.write(&run.id, &entry)
    }

    async fn check_out(&self, container: &dyn Container, t: &Target, base: &str) -> Result<()> {
        let mut exec = Exec::new(clone_command(&t.container_repo_url, base), SETUP_TIMEOUT);
        let token = t.access.read_token().await?;
        if let Some(token) = &token {
            exec.envs = repo::token_env(token);
        }
        exec.envs
            .insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());
        let out = container.exec(&exec).await;
        // Whatever became of the clone: it is the token's one use.
        if let Some(token) = &token
            && let Err(e) = t.access.revoke_read(token).await
        {
            let detail = self.safe(&format!("{e:#}"));
            tracing::warn!(detail, "ending a read token failed");
        }
        let out = out?;
        if out.exit_code != 0 {
            let said = format!("the container's clone failed: {}", out.output.trim());
            if repo::refuses_access(&out.output) {
                return Err(AccessRefused(said).into());
            }
            bail!("{said}");
        }
        Ok(())
    }

    /// Takes back the container of a run this process holds nothing for:
    /// its `RunBox` when there is one, `None` when the run never had one. An
    /// `Err` is the run's to see. The ledger names the container; a run it
    /// does not name may still have one, from a crash just after making it.
    async fn recover(&self, run: &Run, entry: &Entry) -> Result<Option<RunBox>, HostError> {
        if let Some(id) = &entry.container {
            let known = Found {
                id: id.clone(),
                labels: entry.labels.clone(),
            };
            return self.reattach(run, entry, &known).await;
        }
        let found = self
            .runtime
            .find(&self.labels_of(run))
            .await
            .map_err(|e| self.unavailable(run, &e))?;
        match found.as_slice() {
            [] => Ok(None),
            [one] => self.reattach(run, entry, one).await,
            _ => Err(lost(run, "it has more than one container")),
        }
    }

    /// Takes back the container `found`: `None` when it holds nothing of the
    /// run's, its first clone cut off, and is ended for a fresh one.
    async fn reattach(
        &self,
        run: &Run,
        entry: &Entry,
        found: &Found,
    ) -> Result<Option<RunBox>, HostError> {
        let container = self
            .runtime
            .attach(&found.id, &self.spec)
            .await
            .map_err(|e| {
                if is_gone(&e) {
                    lost(run, &self.safe(&format!("{e:#}")))
                } else {
                    self.unavailable(run, &e)
                }
            })?;
        let Some(base) = found.labels.get(LABEL_BASE) else {
            return Ok(Some(RunBox {
                container,
                checkout: None,
                orphan: None,
            }));
        };
        if !is_commit_id(base) {
            return Err(lost(run, "its base label is not a commit"));
        }
        let check = Exec::new(
            format!("git -C {REPO_DIR} cat-file -e {base}^{{commit}}"),
            CHECK_TIMEOUT,
        );
        let out = container
            .exec(&check)
            .await
            .map_err(|e| self.unavailable(run, &e))?;
        if out.exit_code != 0 {
            if entry.checked_out {
                return Err(lost(run, "its container holds no checkout"));
            }
            if let Err(e) = self.runtime.destroy(&found.id).await {
                tracing::warn!(error = %e, run = run.id, "destroying a container failed");
            }
            self.ledger.remove(&run.id);
            return Ok(None);
        }
        let thread = run.thread_ids.first();
        let thread = thread.ok_or_else(|| lost(run, "it has no thread"))?;
        let branch = entry.branch.clone();
        let branch = branch.unwrap_or_else(|| thread_branch(thread));
        let queue = found.labels.get(LABEL_QUEUE).map(String::as_str);
        let target = self.resolved(queue).await;
        let target = target.map_err(|e| self.unavailable(run, &e))?;
        // A target that stands on another repository now is gone, as far as
        // this checkout goes: its change is not for what the target holds.
        let same = |t: &Arc<Target>| entry.repository.as_ref() == Some(&t.repository());
        let Some(target) = target.filter(same) else {
            return Ok(Some(RunBox {
                container,
                checkout: None,
                orphan: Some(branch),
            }));
        };
        let host = self
            .host_clone(&target)
            .await
            .map_err(|e| self.unavailable(run, &e))?;
        let mut pushed: Vec<&str> = entry.pushed.iter().map(String::as_str).collect();
        pushed.extend(entry.pending.iter().map(|p| p.commit.as_str()));
        let why = host
            .clone
            .resume_branch(&branch, base, &pushed)
            .await
            .map_err(|e| self.unavailable(run, &e))?;
        if let Some(why) = why {
            return Err(lost(run, &why));
        }
        Ok(Some(RunBox {
            container,
            checkout: Some(Checkout {
                target,
                branch,
                base: base.clone(),
                clone: host.clone,
                _dir: host.dir,
            }),
            orphan: None,
        }))
    }

    /// The change in the container against `base`, or why it is refused.
    /// Bytes from a container are only ever a patch for
    /// [`repo::Clone::apply_onto`], whatever exit code came with them.
    async fn take_patch(
        container: &dyn Container,
        base: &str,
    ) -> Result<Result<Vec<u8>, &'static str>> {
        let exec = Exec::new(patch_command(base), PATCH_TIMEOUT).in_dir(REPO_DIR);
        let out = container.exec(&exec).await?;
        if out.output.len() >= MAX_EXEC_OUTPUT {
            return Ok(Err("the change is over the size limit"));
        }
        if out.exit_code != 0 {
            return Ok(Err("the change could not be read from the container"));
        }
        let clean: String = out.output.split_whitespace().collect();
        let decoded = base64::engine::general_purpose::STANDARD.decode(clean);
        Ok(decoded.map_err(|_| "what the container gave is not a change"))
    }

    fn stop_wait(&self, run: &str) {
        let mut held = self.waits.lock().expect("the waits' lock");
        if let Some((_, earlier)) = held.remove(run) {
            earlier.abort();
        }
    }

    /// Watches the preview of `pr`, in place of any earlier wait of this
    /// run, and tells the sink. It depends on nothing of the run's, and
    /// [`Preview::wait`]'s deadline ends it.
    fn watch_preview(
        &self,
        run: &str,
        preview: &Preview,
        http: &reqwest::Client,
        pr: u64,
        commit: &str,
        reviewed: bool,
    ) {
        let Some(sink) = self.sink.lock().expect("the sink's lock").clone() else {
            return;
        };
        self.stop_wait(run);
        let number = self.wait_number.fetch_add(1, Ordering::Relaxed);
        let (id, preview, commit) = (run.to_string(), preview.clone(), commit.to_string());
        let (http, waits) = (http.clone(), self.waits.clone());
        let mut held = self.waits.lock().expect("the waits' lock");
        let task = tokio::spawn(async move {
            let told = if preview.wait(&http, pr, &commit).await {
                sink.preview_ready(&id, &preview.url(pr), reviewed).await
            } else {
                sink.preview_missed(&id).await
            };
            if let Err(e) = told {
                tracing::warn!(error = %e.message, run = id, "telling of a preview failed");
            }
            let mut held = waits.lock().expect("the waits' lock");
            if held.get(&id).is_some_and(|(n, _)| *n == number) {
                held.remove(&id);
            }
        });
        held.insert(run.to_string(), (number, task.abort_handle()));
    }

    /// Tells the sink a wait that cannot go on ended with no preview seen.
    fn tell_missed(&self, run: &str) {
        let Some(sink) = self.sink.lock().expect("the sink's lock").clone() else {
            return;
        };
        let id = run.to_string();
        tokio::spawn(async move {
            if let Err(e) = sink.preview_missed(&id).await {
                tracing::warn!(error = %e.message, run = id, "telling of a preview failed");
            }
        });
    }

    /// Takes up run `id`'s wait from `entry`, if it has one, for what is left
    /// of it: one past its end looks once more ([`Preview::wait`] always
    /// looks). A wait whose target has gone, or that is past its end with its
    /// target out of reach, stops with no look, which is told as any wait
    /// that saw no preview. Told twice of a preview or of none, a sink adds
    /// nothing.
    fn take_up_wait(&self, id: &str, entry: &Entry) {
        let Some(a) = entry.awaiting.as_ref() else {
            return;
        };
        let left = a.until.saturating_sub(unix_now());
        let queue = entry.labels.get(LABEL_QUEUE).map(String::as_str);
        match self.described(queue) {
            Ok(Some(d)) => {
                let mut preview = d.preview;
                preview.timeout = Duration::from_secs(left);
                let reviewed = reviewed_of(entry);
                self.watch_preview(id, &preview, &d.preview_http, a.pr, &a.commit, reviewed);
            }
            Ok(None) => self.tell_missed(id),
            Err(e) => {
                tracing::warn!(error = %e, run = id, "taking up a preview's wait failed");
                if left == 0 {
                    self.tell_missed(id);
                }
            }
        }
    }

    /// Takes up the waits the ledger holds.
    fn rearm_waits(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            tracing::warn!("no runtime to take up preview waits on");
            return;
        }
        for id in self.ledger.runs() {
            if let Ok(entry) = self.ledger.read(&id) {
                self.take_up_wait(&id, &entry);
            }
        }
    }

    /// Brings the clone up to a branch only the forge has moved. A tip that
    /// is none of the run's is refused, as `git push` would refuse it.
    async fn sync_with_forge(
        &self,
        run: &Run,
        entry: &Entry,
        c: &Checkout,
    ) -> Result<(), HostError> {
        let down = |e: anyhow::Error| self.unavailable(run, &e);
        let token = c.target.access.write_token();
        let token = token.await.map_err(down)?;
        c.clone
            .fetch_branch(&c.branch, token.as_ref())
            .await
            .map_err(down)?;
        let mut known: Vec<&str> = entry.pushed.iter().map(String::as_str).collect();
        known.extend(entry.pending.iter().map(|p| p.commit.as_str()));
        let why = c
            .clone
            .resume_branch(&c.branch, &c.base, &known)
            .await
            .map_err(down)?;
        match why {
            Some(why) => Err(HostError::new(CODE_CHANGE_REFUSED, why)),
            None => Ok(()),
        }
    }

    /// Has the forge commit the judged index and move the branch to it;
    /// answers the commit. Unless `t.unsigned` allows it, a commit the forge
    /// does not verify stops here, on no branch, and is not remembered. The
    /// ledger hears of any other before the branch moves, and the same tree
    /// on the same parent, under any key, takes that commit. A crash between
    /// the forge's answer and the ledger leaves a commit on no branch, which
    /// nothing sees.
    async fn commit_by_forge(
        &self,
        run: &Run,
        c: &Checkout,
        key: &str,
        message: &str,
    ) -> Result<String, HostError> {
        let down = |e: anyhow::Error| self.unavailable(run, &e);
        let t = c.target.as_ref();
        let parent = c.clone.head().await.map_err(down)?;
        let base_tree = c.clone.tree_of("HEAD").await.map_err(down)?;
        let tree = c.clone.index_tree().await.map_err(down)?;
        if tree == base_tree {
            return Ok(parent);
        }
        let mut entry = self.ledger.read(&run.id).map_err(down)?;
        let made = entry
            .pending
            .as_ref()
            .filter(|p| p.tree == tree && p.parent == parent)
            .map(|p| p.commit.clone());
        let commit = match made {
            Some(commit) => commit,
            None => {
                let changes = c.clone.tree_changes(&t.policy).await.map_err(down)?;
                let changes =
                    changes.map_err(|m| HostError::new(CODE_CHANGE_REFUSED, self.safe(&m)))?;
                let new = NewCommit {
                    parent,
                    base_tree,
                    tree: tree.clone(),
                    changes,
                    message: message.to_string(),
                };
                let by_forge = |e: anyhow::Error| self.by_forge(run, t, &e);
                let made = t.forge.commit(&new).await.map_err(by_forge)?;
                if !made.verified && t.unsigned != UnsignedCommits::Allow {
                    return Err(HostError::new(
                        CODE_CHANGE_REFUSED,
                        "the forge did not sign this commit and this repository takes no \
                         unsigned commits, so nothing was published. Report that instead of \
                         proposing it again.",
                    ));
                }
                let commit = made.id;
                entry.pending = Some(Pending {
                    key: key.to_string(),
                    commit: commit.clone(),
                    tree,
                    parent: new.parent,
                });
                self.ledger.write(&run.id, &entry).map_err(down)?;
                commit
            }
        };
        let exists = c.clone.remote_tip(&c.branch).await.map_err(down)?;
        t.forge
            .advance(&c.branch, &commit, exists.is_some())
            .await
            .map_err(|e| self.by_forge(run, t, &e))?;
        entry.pushed.push(commit.clone());
        self.ledger
            .write(&run.id, &entry)
            .map_err(|e| self.after_push(run, &e))?;
        Ok(commit)
    }

    /// Whether the target takes one more push, counting it, and one more pull
    /// request if this branch would open one. The same proposal coming back
    /// to finish has been counted.
    async fn within_caps(&self, run: &Run, c: &Checkout, key: &str) -> Result<(), HostError> {
        let down = |e: anyhow::Error| self.unavailable(run, &e);
        let t = c.target.as_ref();
        let entry = self.ledger.read(&run.id).map_err(down)?;
        if entry.pending.is_some_and(|p| p.key == key) {
            return Ok(());
        }
        let most = t.policy.max_open_pull_requests;
        if t.forge.find_open(&c.branch).await.map_err(down)?.is_none()
            && t.forge.open_under(BRANCH_PREFIX).await.map_err(down)? >= most
        {
            return Err(HostError::new(
                CODE_CHANGE_REFUSED,
                format!(
                    "this repository holds {most} open pull requests from ClaxBot, the most it \
                     takes, so nothing was published. A person must merge or close one first. \
                     Report that instead of proposing it again."
                ),
            ));
        }
        let most = t.policy.max_pushes_per_day;
        let counted = self.ledger.take_push(&t.repo_url, most, unix_now());
        if !counted.map_err(down)? {
            return Err(HostError::new(
                CODE_CHANGE_REFUSED,
                format!(
                    "this repository took {most} pushes from ClaxBot in the last 24 hours, the \
                     most it takes, so nothing was published. Report that instead of proposing \
                     it again."
                ),
            ));
        }
        Ok(())
    }

    /// Commits `patch` and pushes it, or has the forge do both; answers the
    /// commit. The ledger hears of the commit before the push.
    async fn publish(
        &self,
        run: &Run,
        c: &Checkout,
        patch: &[u8],
        key: &str,
        title: &str,
    ) -> Result<String, HostError> {
        let refused = |m: String| HostError::new(CODE_CHANGE_REFUSED, self.safe(&m));
        let down = |e: anyhow::Error| self.unavailable(run, &e);
        let t = c.target.as_ref();
        if patch.is_empty() {
            return Err(refused("the working tree has no change".to_string()));
        }
        let why = c
            .clone
            .apply_onto(&c.base, patch, &t.policy)
            .await
            .map_err(down)?;
        if let Some(why) = why {
            return Err(refused(why));
        }
        self.within_caps(run, c, key).await?;
        let thread = run.thread_ids.first().map_or("", String::as_str);
        let message = format!("{title}\n\nReport: {thread}\n");
        if t.forge.commits() {
            return self.commit_by_forge(run, c, key, &message).await;
        }
        if t.unsigned != UnsignedCommits::Allow {
            return Err(refused(
                "a commit pushed from here is unsigned and this repository takes no unsigned \
                 commits, so nothing was published. Report that instead of proposing it again."
                    .to_string(),
            ));
        }
        let commit = c.clone.commit(&message).await.map_err(down)?;
        let mut entry = self.ledger.read(&run.id).map_err(down)?;
        entry.pending = Some(Pending {
            key: key.to_string(),
            commit: commit.clone(),
            ..Pending::default()
        });
        self.ledger.write(&run.id, &entry).map_err(down)?;
        let token = t.access.write_token().await.map_err(down)?;
        c.clone
            .push(&c.branch, token.as_ref())
            .await
            .map_err(|e| self.by_push(run, &e))?;
        entry.pushed.push(commit.clone());
        self.ledger
            .write(&run.id, &entry)
            .map_err(|e| self.after_push(run, &e))?;
        Ok(commit)
    }

    /// Ends the containers of runs that are over: every container of this
    /// workspace but those of `active` and of runs this process holds.
    /// Answers how many. `active` must name each run that may yet resume. An
    /// entry stays while its preview's wait has time left, for a restart.
    pub async fn end_finished(&self, active: &[String]) -> Result<usize> {
        let active: HashSet<&String> = active.iter().collect();
        if self.spec.labels.is_empty() {
            bail!("a workspace with no labels of its own cannot tell its containers");
        }
        let found = self.runtime.find(&self.spec.labels).await?;
        let held: HashSet<String> = {
            let runs = self.runs.lock().expect("the runs' lock");
            runs.keys().cloned().collect()
        };
        let over = |id: &String| !active.contains(id) && !held.contains(id);
        let mut ended = 0;
        for f in &found {
            let Some(run) = f.labels.get(LABEL_RUN) else {
                continue;
            };
            if !over(run) {
                continue;
            }
            match self.runtime.destroy(&f.id).await {
                Ok(()) => ended += 1,
                Err(e) => tracing::warn!(error = %e, run, "destroying a container failed"),
            }
        }
        let now = unix_now();
        let waiting = |id: &str| {
            let entry = self.ledger.read(id);
            entry.is_ok_and(|e| e.awaiting.is_some_and(|a| a.until > now))
        };
        for id in self.ledger.runs() {
            if over(&id) && !waiting(&id) {
                self.ledger.remove(&id);
            }
        }
        Ok(ended)
    }

    /// Ends whatever the runtime has of a run; answers whether it had any.
    async fn sweep(&self, run: &Run) -> bool {
        let found = match self.runtime.find(&self.labels_of(run)).await {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(error = %e, run = run.id, "listing a run's containers failed");
                return false;
            }
        };
        for f in &found {
            if let Err(e) = self.runtime.destroy(&f.id).await {
                tracing::warn!(error = %e, run = run.id, "destroying a container failed");
            }
        }
        !found.is_empty()
    }
}

#[async_trait::async_trait]
impl Workspace for ContainerWorkspace {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        let unsure = |e: &anyhow::Error| {
            let detail = self.safe(&format!("{e:#}"));
            tracing::warn!(run = run.id, detail, "a run's target could not be read");
            Offer::unsure()
        };
        match self.ledger.read(&run.id) {
            Ok(Entry {
                offer: Some(kept),
                finished: false,
                ..
            }) => {
                let confidential = kept.confidential || self.confidential_now(run, queue);
                return Offer {
                    confidential,
                    ..kept
                };
            }
            Ok(_) => {}
            Err(e) => return unsure(&e),
        }
        let t = match self.described(queue) {
            Ok(t) => t,
            Err(e) => return unsure(&e),
        };
        let Some(t) = t else {
            return if self.shell_for_all {
                Offer::SHELL
            } else {
                Offer::default()
            };
        };
        change_offer(
            t.confidential,
            t.reviewed_previews,
            t.wake_on_reply,
            t.follow_ups,
            t.guidance,
            t.preview_gate,
        )
    }

    fn target_private(&self, run: &Run, queue: Option<&str>) -> bool {
        let kept = self.ledger.read(&run.id).ok().and_then(|e| e.offer);
        if kept.is_some_and(|o| o.confidential) {
            return true;
        }
        let Some(q) = queue else {
            return false;
        };
        if let Some(t) = self.file_target(q) {
            return t.confidential;
        }
        let said = self.source.as_ref().map(|s| s.confidential(q));
        said.is_some_and(|said| said.unwrap_or(false))
    }

    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        let slot = self.slot(run);
        let mut held = slot.lock().await;
        let entry = self.entry(run)?;
        if held.is_none() {
            *held = self.recover(run, &entry).await?;
            if let Some(b) = held.as_ref().filter(|b| b.orphan.is_none()) {
                let has = b.checkout.as_ref().map(|c| c.target.queue.as_str());
                let wants = self.described(queue);
                let wants = wants.map_err(|e| self.unavailable(run, &e))?;
                if has != queue.filter(|q| wants.is_some() || has == Some(q)) {
                    *held = None;
                    return Err(lost(run, "its container is another queue's"));
                }
            }
        }
        if held.is_none() {
            let made = self.start(run, queue).await;
            *held = Some(made.map_err(|e| self.unavailable(run, &e))?);
        }
        let b = held.as_ref().expect("just made");
        let mut exec = Exec::new(command, Duration::from_secs(timeout_secs));
        exec.cwd = cwd.map(str::to_string);
        if b.checkout.is_some() || b.orphan.is_some() {
            exec.envs = BTreeMap::from([("CLAX_REPO".to_string(), REPO_DIR.to_string())]);
            exec.cwd.get_or_insert_with(|| REPO_DIR.to_string());
        }
        match b.container.exec(&exec).await {
            Ok(out) => Ok(json!({"exit_code": out.exit_code, "output": self.safe(&out.output)})),
            Err(e) => {
                // It may have stopped: the next call takes it back. This
                // command may have run, so it is not repeated here.
                *held = None;
                let detail = self.safe(&format!("{e:#}"));
                tracing::warn!(run = run.id, detail, "a command got no answer");
                Err(HostError::new(
                    CODE_SHELL_UNAVAILABLE,
                    "the container gave no answer and the command may or may not have run. \
                     Look before running it again.",
                ))
            }
        }
    }

    async fn propose(&self, run: &Run, proposal: &Proposal) -> Result<Value, HostError> {
        let slot = self.slot(run);
        let mut held = slot.lock().await;
        let entry = self.entry(run)?;
        if let Some(done) = entry.proposals.get(&proposal.key) {
            self.take_up_wait(&run.id, &entry);
            return Ok(done.clone());
        }
        if held.is_none() {
            *held = self.recover(run, &entry).await?;
        }
        let refused = |m: &str| HostError::new(CODE_CHANGE_REFUSED, m);
        let down = |e: anyhow::Error| self.unavailable(run, &e);
        let b = held
            .as_ref()
            .ok_or_else(|| refused("nothing has run in the container yet"))?;
        if let Some(branch) = &b.orphan {
            return Err(refused(&format!(
                "this run's repository was disconnected or replaced while it was interrupted, \
                 and its credentials went with it, so nothing more can be published. Anything \
                 already pushed stands on {branch}, with no pull request if it had none. Report \
                 that instead of proposing it again."
            )));
        }
        let c = b
            .checkout
            .as_ref()
            .ok_or_else(|| refused("this run has no repository"))?;
        let t = c.target.as_ref();
        let title: String = strip_html_comments(&self.safe(&proposal.title))
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .chars()
            .take(100)
            .collect();
        let title = commit_subject(&title);

        if t.forge.commits() {
            self.sync_with_forge(run, &entry, c).await?;
        }

        let _proposing = self.proposing.lock().await;
        // A step executed again: this key's push landed if the remote stands
        // on its commit, and only the pull request may be missing. A commit
        // here that no push carried is not that.
        let tip = c.clone.remote_tip(&c.branch).await.map_err(down)?;
        let landed = entry
            .pending
            .as_ref()
            .filter(|p| p.key == proposal.key && Some(&p.commit) == tip.as_ref());
        let commit = match landed {
            Some(p) => p.commit.clone(),
            None => {
                self.stop_wait(&run.id);
                let patch = match Self::take_patch(b.container.as_ref(), &c.base).await {
                    Ok(patch) => patch.map_err(refused)?,
                    Err(e) => {
                        // It may have stopped: the next call takes it back.
                        let answer = down(e);
                        *held = None;
                        return Err(answer);
                    }
                };
                let published = self.publish(run, c, &patch, &proposal.key, &title).await;
                match published {
                    Ok(commit) => {
                        self.worked(t, RepoUse::Write);
                        commit
                    }
                    Err(e) => {
                        if e.code == CODE_SHELL_UNAVAILABLE {
                            self.failed(t, PUBLISH_FAILED);
                        }
                        return Err(e);
                    }
                }
            }
        };
        let pushed = |e: anyhow::Error| self.after_push(run, &e);
        let thread = run.thread_ids.first().map_or("", String::as_str);
        let found = t.forge.find_open(&c.branch).await.map_err(pushed)?;
        let paths = c.clone.changed_from(&t.base).await.map_err(pushed)?;
        let flagged: Vec<&String> = paths.iter().filter(|p| t.policy.sensitive(p)).collect();
        let report = report_name(&t.policy, thread);
        let pr = match found {
            Some(pr) => {
                let block = defuse_mentions(&host_block(&paths, &flagged, &report));
                let current = t.forge.body(pr.number).await.map_err(pushed)?;
                let body = with_host_block(&current, &block);
                t.forge.describe(pr.number, &body).await.map_err(pushed)?;
                pr
            }
            None => t
                .forge
                .open(&NewPullRequest {
                    title: title.clone(),
                    body: pull_request_body(
                        &paths,
                        &flagged,
                        &report,
                        &self.safe(&proposal.summary),
                    ),
                    head: c.branch.clone(),
                    base: t.base.clone(),
                })
                .await
                .map_err(pushed)?,
        };
        let done = proposed(&pr, &commit);
        let mut entry = self.ledger.read(&run.id).map_err(pushed)?;
        entry.pending = None;
        entry.proposals.insert(proposal.key.clone(), done.clone());
        entry.awaiting = Some(Awaiting {
            pr: pr.number,
            commit: commit.clone(),
            until: unix_now() + t.preview.timeout.as_secs(),
        });
        self.ledger.write(&run.id, &entry).map_err(pushed)?;
        let (reviewed, http) = (reviewed_of(&entry), &t.reach.preview);
        self.watch_preview(&run.id, &t.preview, http, pr.number, &commit, reviewed);
        Ok(done)
    }

    async fn reap(&self, active: &[String]) {
        match self.end_finished(active).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(n, "ended the containers of finished runs"),
            Err(e) => tracing::warn!(error = %e, "ending finished runs' containers failed"),
        }
    }

    fn previews(&self, sink: Arc<dyn PreviewSink>) {
        *self.sink.lock().expect("the sink's lock") = Some(sink);
        self.rearm_waits();
    }

    async fn finish(&self, run: &Run) {
        // With the slot held throughout: a call waiting on it then finds the
        // run marked, and makes nothing.
        let slot = self.slot(run);
        let mut held = slot.lock().await;
        held.take();
        // Whatever was known of it: a container made just before a crash is
        // in no ledger, and a preview's wait goes on regardless.
        let swept = self.sweep(run).await;
        let mut entry = self.ledger.read(&run.id).unwrap_or_default();
        // A run that never had a container leaves no file behind it.
        if swept || entry != Entry::default() {
            if let Some(id) = entry.container.take()
                && let Err(e) = self.runtime.destroy(&id).await
            {
                tracing::warn!(error = %e, run = run.id, "destroying a container failed");
            }
            entry.finished = true;
            if let Err(e) = self.ledger.write(&run.id, &entry) {
                tracing::warn!(error = %e, run = run.id, "marking a run finished failed");
            }
        }
        drop(held);
        self.runs.lock().expect("the runs' lock").remove(&run.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_command_keeps_stderr_out_of_the_base64_and_cleans_up() {
        let c = patch_command("abc");
        assert!(c.contains("2>\"$f.err\""), "{c}");
        assert!(c.contains("git diff --cached --binary abc"), "{c}");
        assert!(c.contains("trap 'rm -f"), "{c}");
    }

    #[test]
    fn a_clone_command_quotes_its_url() {
        let c = clone_command("https://x/it's.git", "abc");
        assert!(c.contains(r"'https://x/it'\''s.git'"), "{c}");
        assert!(c.ends_with("--detach abc"), "{c}");
    }

    #[test]
    fn only_a_full_commit_id_reads_as_one() {
        assert!(is_commit_id(&"a1".repeat(20)));
        assert!(!is_commit_id("a1b2"));
        assert!(!is_commit_id(&format!("{};rm", "a".repeat(40))));
    }
}

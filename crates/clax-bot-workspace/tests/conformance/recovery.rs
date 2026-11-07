//! `ContainerWorkspace` over a runtime whose containers are directories: a
//! daemon's restart is a new workspace on the same runtime, and a real `git`
//! stands on both sides. What a hostile container can do is here too, since
//! a container's answers are whatever a test says they are.

use crate::fixture::{basic, git, seed, serve};
use anyhow::{Result, bail};
use clax_bot::workspace::{
    CODE_CHANGE_REFUSED, CODE_SHELL_UNAVAILABLE, PreviewSink, Proposal, Workspace,
};
use clax_bot_sandbox::HostError;
use clax_bot_workspace::forge::{Forge, NewPullRequest, PullRequest, UnsignedCommits};
use clax_bot_workspace::ledger::{Awaiting, Entry, Ledger};
use clax_bot_workspace::net::Reach;
use clax_bot_workspace::preview::Preview;
use clax_bot_workspace::repo::{Policy, Redactor, RepoAccess, StaticAccess, Token};
use clax_bot_workspace::runtime::{
    Container, ContainerRuntime, ContainerSpec, Exec, ExecOutput, Found, Gone,
};
use clax_bot_workspace::workspace::{ContainerWorkspace, LABEL_BASE, REPO_DIR, Target};
use clax_store::bot_runs::{RequestKind, Run, RunStatus};
use clax_store::credits::KeySource;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

/// What the runtime holds. Outliving a workspace is the point of it.
#[derive(Default)]
pub(crate) struct Boxes {
    live: HashMap<String, (PathBuf, BTreeMap<String, String>)>,
    made: usize,
    /// Every command run, with its environment.
    pub(crate) execs: Vec<Exec>,
    /// Answers `git diff` commands in place of running them.
    patch_answer: Option<ExecOutput>,
    attach_fails: bool,
    find_fails: bool,
    /// How many times `find` was asked.
    finds: usize,
    /// Every command answers `Err`.
    pub(crate) stopped: bool,
}

#[derive(Clone)]
pub(crate) struct Local {
    root: Arc<TempDir>,
    boxes: Arc<Mutex<Boxes>>,
}

impl Local {
    fn new() -> Local {
        Local {
            root: Arc::new(tempfile::tempdir().unwrap()),
            boxes: Arc::default(),
        }
    }

    pub(crate) fn with<T>(&self, f: impl FnOnce(&mut Boxes) -> T) -> T {
        f(&mut self.boxes.lock().unwrap())
    }

    fn made(&self) -> usize {
        self.with(|b| b.made)
    }

    /// Every command run so far.
    pub(crate) fn execs(&self) -> Vec<Exec> {
        self.with(|b| b.execs.clone())
    }

    fn live(&self) -> usize {
        self.with(|b| b.live.len())
    }
}

struct LocalBox {
    id: String,
    dir: PathBuf,
    boxes: Arc<Mutex<Boxes>>,
}

#[async_trait::async_trait]
impl Container for LocalBox {
    fn id(&self) -> &str {
        &self.id
    }

    async fn exec(&self, exec: &Exec) -> Result<ExecOutput> {
        let canned = {
            let mut b = self.boxes.lock().unwrap();
            if !b.live.contains_key(&self.id) {
                bail!("no such container");
            }
            if b.stopped {
                bail!("stopped");
            }
            b.execs.push(exec.clone());
            b.patch_answer
                .clone()
                .filter(|_| exec.command.contains("git diff"))
        };
        if let Some(answer) = canned {
            return Ok(answer);
        }
        let repo = self.dir.join("repo");
        let repo = repo.to_str().unwrap();
        let cwd = exec.cwd.as_deref().map(|c| c.replace(REPO_DIR, repo));
        let mut c = tokio::process::Command::new("sh");
        c.args([
            "-c",
            &format!("{{ {}\n}} 2>&1", exec.command.replace(REPO_DIR, repo)),
        ])
        .current_dir(cwd.unwrap_or_else(|| self.dir.to_str().unwrap().to_string()))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(&exec.envs);
        let out = c.output().await?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            output: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }
}

#[async_trait::async_trait]
impl ContainerRuntime for Local {
    fn limits_domains(&self) -> bool {
        true
    }

    async fn create(&self, spec: &ContainerSpec) -> Result<Box<dyn Container>> {
        let mut b = self.boxes.lock().unwrap();
        b.made += 1;
        let id = format!("box-{}", b.made);
        let dir = self.root.path().join(&id);
        std::fs::create_dir(&dir)?;
        b.live
            .insert(id.clone(), (dir.clone(), spec.labels.clone()));
        Ok(Box::new(LocalBox {
            id,
            dir,
            boxes: self.boxes.clone(),
        }))
    }

    async fn attach(&self, id: &str, _spec: &ContainerSpec) -> Result<Box<dyn Container>> {
        let mut b = self.boxes.lock().unwrap();
        if b.attach_fails {
            bail!("attaching box-secret-detail failed");
        }
        if b.stopped {
            b.stopped = false;
        }
        let (dir, _) = b.live.get(id).ok_or_else(|| Gone(format!("no {id}")))?;
        Ok(Box::new(LocalBox {
            id: id.to_string(),
            dir: dir.clone(),
            boxes: self.boxes.clone(),
        }))
    }

    async fn find(&self, labels: &BTreeMap<String, String>) -> Result<Vec<Found>> {
        let mut b = self.boxes.lock().unwrap();
        b.finds += 1;
        if b.find_fails {
            bail!("the runtime is down");
        }
        let all = b.live.iter().map(|(id, (_, l))| Found {
            id: id.clone(),
            labels: l.clone(),
        });
        Ok(all.filter(|f| f.labels_hold(labels)).collect())
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        self.boxes.lock().unwrap().live.remove(id);
        Ok(())
    }
}

#[derive(Default)]
struct FakeForge {
    opened: Mutex<Vec<NewPullRequest>>,
    /// Fails every call with this text.
    fails_with: Mutex<Option<String>>,
    /// Fails opening a pull request alone, so the push has gone before it.
    open_fails: Mutex<bool>,
    /// What each [`Forge::describe`] said.
    described: Mutex<Vec<String>>,
    /// What a person made of the pull request's text.
    edited: Mutex<Option<String>>,
    /// Open pull requests of others of ClaxBot's branches.
    others_open: Mutex<usize>,
}

#[async_trait::async_trait]
impl Forge for FakeForge {
    async fn find_open(&self, head: &str) -> Result<Option<PullRequest>> {
        if let Some(text) = self.fails_with.lock().unwrap().clone() {
            bail!("{text}");
        }
        let opened = self.opened.lock().unwrap();
        Ok(opened.iter().any(|p| p.head == head).then(pull_request))
    }

    async fn open(&self, pr: &NewPullRequest) -> Result<PullRequest> {
        if *self.open_fails.lock().unwrap() {
            bail!("the forge is down");
        }
        self.opened.lock().unwrap().push(pr.clone());
        Ok(pull_request())
    }

    async fn body(&self, _number: u64) -> Result<String> {
        if let Some(text) = self.edited.lock().unwrap().clone() {
            return Ok(text);
        }
        let described = self.described.lock().unwrap().last().cloned();
        let opened = self.opened.lock().unwrap().last().map(|p| p.body.clone());
        Ok(described.or(opened).unwrap_or_default())
    }

    async fn describe(&self, _number: u64, body: &str) -> Result<()> {
        self.described.lock().unwrap().push(body.to_string());
        *self.edited.lock().unwrap() = None;
        Ok(())
    }

    async fn open_under(&self, prefix: &str) -> Result<usize> {
        assert_eq!(prefix, "claxbot/");
        let opened = self.opened.lock().unwrap().len();
        Ok(opened + *self.others_open.lock().unwrap())
    }
}

fn pull_request() -> PullRequest {
    PullRequest {
        number: 1,
        html_url: "https://forge.example/pr/1".to_string(),
    }
}

pub(crate) fn run(id: &str) -> Run {
    Run {
        id: id.to_string(),
        artifact_id: "a".to_string(),
        person: "p".to_string(),
        request: RequestKind::Thread,
        thread_ids: vec!["thread1".to_string()],
        status: RunStatus::Running,
        reason: None,
        model: "m".to_string(),
        key_source: KeySource::Instance,
        base_version: 1,
        credits_milli: 0,
        claimed_by: None,
        claimed_at: None,
        created_at: String::new(),
        updated_at: String::new(),
        ended_at: None,
        due_at: None,
        agent_grant: None,
        web: String::new(),
        confidential: false,
        woken_by: None,
    }
}

const QUEUE: &str = "q";

/// A repository, a runtime and a ledger that outlive any one workspace.
pub(crate) struct World {
    pub(crate) root: TempDir,
    pub(crate) bare: PathBuf,
    pub(crate) runtime: Local,
    forge: Arc<FakeForge>,
    url: String,
    write_token: Option<Token>,
    read_token: Option<Token>,
    pub(crate) secrets: Arc<Redactor>,
}

impl World {
    pub(crate) fn new() -> World {
        let root = tempfile::tempdir().unwrap();
        let bare = seed(root.path());
        let url = format!("file://{}", bare.display());
        World {
            root,
            bare,
            runtime: Local::new(),
            forge: Arc::default(),
            url,
            write_token: None,
            read_token: None,
            secrets: Arc::default(),
        }
    }

    pub(crate) fn target(&self) -> Target {
        Target {
            stamp: None,
            queue: QUEUE.to_string(),
            repo_url: self.url.clone(),
            container_repo_url: self.url.clone(),
            base: "main".to_string(),
            access: Arc::new(StaticAccess {
                read: self.read_token.clone(),
                write: self.write_token.clone(),
            }),
            confidential: false,
            reviewed_previews: false,
            wake_on_reply: false,
            unsigned: UnsignedCommits::Allow,
            policy: Policy::default(),
            forge: self.forge.clone(),
            preview: Preview {
                url_template: "http://127.0.0.1:1/{pr}".to_string(),
                ready_path: "/".to_string(),
                ready_names_commit: false,
                timeout: Duration::ZERO,
                poll: Duration::ZERO,
            },
            reach: Reach::any(),
        }
    }

    /// A daemon, new: what it knows of runs is what the runtime and the
    /// ledger say.
    fn daemon(&self) -> ContainerWorkspace {
        let spec = ContainerSpec {
            labels: BTreeMap::from([("clax.workspace".to_string(), "here".to_string())]),
            ..ContainerSpec::default()
        };
        ContainerWorkspace::keeping(
            Arc::new(self.runtime.clone()),
            spec,
            vec![self.target()],
            true,
            self.root.path().join("ledger"),
            self.secrets.clone(),
        )
    }

    pub(crate) fn pushed(&self, rev: &str) -> String {
        git(&self.bare, &["rev-parse", rev])
    }

    /// Pushes a commit of someone else's onto `branch`.
    pub(crate) fn someone_pushes(&self, branch: &str, from: &str) {
        let dir = self.root.path().join(format!("someone-{}", branch.len()));
        git(
            self.root.path(),
            &[
                "clone",
                "--quiet",
                "--branch",
                from,
                &self.url,
                dir.to_str().unwrap(),
            ],
        );
        std::fs::write(dir.join("theirs.txt"), "theirs\n").unwrap();
        git(&dir, &["add", "theirs.txt"]);
        git(&dir, &["commit", "--quiet", "-m", "theirs"]);
        git(
            &dir,
            &["push", "--quiet", "origin", &format!("HEAD:{branch}")],
        );
    }
}

pub(crate) async fn sh(w: &ContainerWorkspace, r: &Run, command: &str) -> Result<Value, HostError> {
    w.exec(r, Some(QUEUE), command, None, 30).await
}

pub(crate) async fn edit(w: &ContainerWorkspace, r: &Run, text: &str) {
    let answer = sh(w, r, &format!("printf '{text}' > app.txt"))
        .await
        .unwrap();
    assert_eq!(answer["exit_code"], 0, "{answer}");
}

pub(crate) fn proposal() -> Proposal {
    Proposal {
        title: "a fix".to_string(),
        summary: "fixed".to_string(),
        key: "k".to_string(),
    }
}

/// A proposal of a later step, under a key of its own.
pub(crate) fn another_proposal() -> Proposal {
    Proposal {
        key: "k2".to_string(),
        ..proposal()
    }
}

/// Fails unless `e` tells the run its workspace is gone.
fn is_lost(e: &HostError) {
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(e.message.contains("workspace was lost"), "{}", e.message);
}

pub(crate) fn file_at(bare: &Path, rev: &str, path: &str) -> String {
    git(bare, &["show", &format!("{rev}:{path}")])
}

const BRANCH: &str = "claxbot/thread1";

#[tokio::test]
async fn a_restarted_daemon_finds_a_runs_container_and_its_edits() {
    let world = World::new();
    let r = run("r1");
    edit(&world.daemon(), &r, "edited\n").await;

    let again = world.daemon();
    let answer = sh(&again, &r, "cat app.txt").await.unwrap();
    assert_eq!(answer["output"], "edited\n");
    assert_eq!(world.runtime.made(), 1, "no second container");
}

#[tokio::test]
async fn a_restarted_daemon_proposes_what_the_container_holds() {
    let world = World::new();
    let r = run("r1");
    let main = world.pushed("main");
    edit(&world.daemon(), &r, "edited\n").await;
    // main moves on: the checkout's base is what the container started from.
    world.someone_pushes("main", "main");

    let answer = world.daemon().propose(&r, &proposal()).await.unwrap();
    assert_eq!(answer["commit"], world.pushed(BRANCH));
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "edited");
    assert_eq!(world.pushed(&format!("{BRANCH}~1")), main);
}

#[tokio::test]
async fn a_bare_shell_comes_back_too() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    w.exec(&r, None, "echo kept > f", None, 30).await.unwrap();
    let again = world.daemon();
    let answer = again.exec(&r, None, "cat f", None, 30).await.unwrap();
    assert_eq!(answer["output"], "kept\n");
    assert_eq!(world.runtime.made(), 1);
}

#[tokio::test]
async fn a_push_nobody_signed_is_refused_unless_the_target_allows_it() {
    let world = World::new();
    let target = Target {
        unsigned: UnsignedCommits::Refuse,
        ..world.target()
    };
    let w = ContainerWorkspace::new(
        Arc::new(world.runtime.clone()),
        ContainerSpec::default(),
        vec![target],
        false,
    );
    let r = run("r1");
    edit(&w, &r, "fixed\n").await;
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
    assert!(e.message.contains("unsigned"), "{}", e.message);
    let refs = git(&world.bare, &["for-each-ref", "--format=%(refname)"]);
    assert_eq!(refs, "refs/heads/main");
}

#[test]
fn a_target_that_opts_in_offers_waking_on_a_reply() {
    let world = World::new();
    let offer = |target: Target| {
        let w = ContainerWorkspace::new(
            Arc::new(world.runtime.clone()),
            ContainerSpec::default(),
            vec![target],
            false,
        );
        w.offer(&run("r1"), Some(QUEUE))
    };
    assert!(!offer(world.target()).wake_on_reply);
    let woken = Target {
        wake_on_reply: true,
        ..world.target()
    };
    assert!(offer(woken).wake_on_reply);
}

/// What an operator writes of a target reaches a run through its offer.
#[test]
fn an_offer_carries_the_targets_guidance_and_preview_gate() {
    let world = World::new();
    let target = Target {
        policy: Policy {
            guidance: Some("Say how.".to_string()),
            preview_gate: Some("press `{commit}`".to_string()),
            ..Policy::default()
        },
        ..world.target()
    };
    let w = ContainerWorkspace::new(
        Arc::new(world.runtime.clone()),
        ContainerSpec::default(),
        vec![target],
        false,
    );
    let offer = w.offer(&run("r1"), Some(QUEUE));
    assert_eq!(offer.guidance.as_deref(), Some("Say how."));
    assert_eq!(offer.preview_gate.as_deref(), Some("press `{commit}`"));
    // A run with no target hears none of it.
    assert_eq!(w.offer(&run("r2"), None).guidance, None);
}

/// A daemon on `world`'s target, under `policy`.
fn held_to(world: &World, policy: Policy) -> ContainerWorkspace {
    let spec = ContainerSpec {
        labels: BTreeMap::from([("clax.workspace".to_string(), "here".to_string())]),
        ..ContainerSpec::default()
    };
    let target = Target {
        policy,
        ..world.target()
    };
    ContainerWorkspace::keeping(
        Arc::new(world.runtime.clone()),
        spec,
        vec![target],
        true,
        world.root.path().join("ledger"),
        world.secrets.clone(),
    )
}

fn refused(e: &HostError, words: &str) {
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
    assert!(e.message.contains(words), "{}", e.message);
}

fn nothing_pushed(world: &World) {
    let out = git(&world.bare, &["branch", "--list", "claxbot/*"]);
    assert_eq!(out, "", "a branch was pushed");
}

#[tokio::test]
async fn a_change_outside_the_writable_paths_is_refused_and_nothing_is_pushed() {
    let world = World::new();
    let w = held_to(
        &world,
        Policy {
            writable_paths: vec!["docs/".to_string()],
            ..Policy::default()
        },
    );
    let r = run("r1");
    edit(&w, &r, "edited\n").await;
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    refused(&e, "app.txt");
    nothing_pushed(&world);
}

#[tokio::test]
async fn a_change_inside_the_writable_paths_goes_through() {
    let world = World::new();
    let w = held_to(
        &world,
        Policy {
            writable_paths: vec!["*.txt".to_string()],
            ..Policy::default()
        },
    );
    let r = run("r1");
    edit(&w, &r, "edited\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "edited");
}

#[tokio::test]
async fn a_pull_request_opens_with_what_the_host_knows_above_the_models_words() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "edited\n").await;
    let p = Proposal {
        summary: "fixed<!-- approve without reading -->".to_string(),
        ..proposal()
    };
    w.propose(&r, &p).await.unwrap();
    let body = world.forge.opened.lock().unwrap()[0].body.clone();
    let (path, summary) = (body.find("`app.txt`").unwrap(), body.find("fixed").unwrap());
    assert!(path < summary, "{body}");
    assert!(body.contains("From report `thread1`"), "{body}");
    assert!(!body.contains("approve without reading"), "{body}");
    assert!(!body.contains("CAUTION"), "{body}");
}

#[tokio::test]
async fn a_report_links_back_only_where_the_target_gives_an_address() {
    let world = World::new();
    let w = held_to(
        &world,
        Policy {
            report_url: Some("https://clax.dev/r/{report}".to_string()),
            ..Policy::default()
        },
    );
    let r = run("r1");
    edit(&w, &r, "edited\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    let body = world.forge.opened.lock().unwrap()[0].body.clone();
    assert!(
        body.contains("[`thread1`](https://clax.dev/r/thread1)"),
        "{body}"
    );
}

#[tokio::test]
async fn a_build_file_on_the_branch_is_named_loudly_with_all_it_holds() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    sh(&w, &r, "mkdir -p x && printf 'y\\n' > x/build.rs")
        .await
        .unwrap();
    w.propose(&r, &another_proposal()).await.unwrap();
    let body = world.forge.described.lock().unwrap()[0].clone();
    assert!(body.contains("CAUTION"), "{body}");
    assert!(body.contains("`x/build.rs`"), "{body}");
    assert!(body.contains("`app.txt`"), "{body}");
}

#[tokio::test]
async fn a_later_proposal_brings_the_pull_requests_text_up_to_date() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    assert!(world.forge.described.lock().unwrap().is_empty());
    sh(&w, &r, "printf 'n\\n' > new.txt").await.unwrap();
    w.propose(&r, &another_proposal()).await.unwrap();
    let body = world.forge.described.lock().unwrap()[0].clone();
    assert!(
        body.contains("`app.txt`") && body.contains("`new.txt`"),
        "{body}"
    );
}

#[tokio::test]
async fn a_persons_words_in_a_pull_request_survive_a_later_proposal() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    let opened = world.forge.opened.lock().unwrap()[0].body.clone();
    *world.forge.edited.lock().unwrap() = Some(format!("{opened}\n\nAsk @sam first."));
    sh(&w, &r, "printf 'n\\n' > new.txt").await.unwrap();
    w.propose(&r, &another_proposal()).await.unwrap();
    let body = world.forge.described.lock().unwrap()[0].clone();
    assert!(body.contains("`new.txt`"), "{body}");
    assert!(body.contains("Ask @sam first."), "{body}");
    assert!(body.contains("> fixed"), "{body}");
}

#[tokio::test]
async fn a_title_closes_nothing_in_the_commit_or_the_pull_request() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    let p = Proposal {
        title: "Fixes #12 <img src=x>".to_string(),
        ..proposal()
    };
    w.propose(&r, &p).await.unwrap();
    let title = world.forge.opened.lock().unwrap()[0].title.clone();
    let subject = git(&world.bare, &["log", "-1", "--format=%s", BRANCH]);
    for text in [title, subject] {
        assert!(!text.contains("#12"), "{text}");
    }
}

/// A run on a thread of its own.
fn run_on(id: &str, thread: &str) -> Run {
    Run {
        thread_ids: vec![thread.to_string()],
        ..run(id)
    }
}

#[tokio::test]
async fn two_runs_at_once_cannot_both_take_the_last_pull_request() {
    let world = World::new();
    let w = held_to(
        &world,
        Policy {
            max_open_pull_requests: 1,
            ..Policy::default()
        },
    );
    let (a, b) = (run_on("r1", "t1"), run_on("r2", "t2"));
    edit(&w, &a, "a\n").await;
    edit(&w, &b, "b\n").await;
    let p = proposal();
    let (first, second) = tokio::join!(w.propose(&a, &p), w.propose(&b, &p));
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(world.forge.opened.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn two_queues_on_one_repository_share_its_days_pushes() {
    let world = World::new();
    let policy = Policy {
        max_pushes_per_day: 1,
        ..Policy::default()
    };
    let other = Target {
        queue: "q2".to_string(),
        policy: policy.clone(),
        ..world.target()
    };
    let first = Target {
        policy,
        ..world.target()
    };
    let w = ContainerWorkspace::keeping(
        Arc::new(world.runtime.clone()),
        ContainerSpec::default(),
        vec![first, other],
        true,
        world.root.path().join("ledger"),
        world.secrets.clone(),
    );
    let (a, b) = (run_on("r1", "t1"), run_on("r2", "t2"));
    edit(&w, &a, "a\n").await;
    w.propose(&a, &proposal()).await.unwrap();
    let wrote = w.exec(&b, Some("q2"), "printf 'b\\n' > app.txt", None, 30);
    wrote.await.unwrap();
    let e = w.propose(&b, &proposal()).await.unwrap_err();
    refused(&e, "pushes");
}

#[tokio::test]
async fn a_new_pull_request_past_the_cap_is_refused_before_anything_is_pushed() {
    let world = World::new();
    *world.forge.others_open.lock().unwrap() = 2;
    let w = held_to(
        &world,
        Policy {
            max_open_pull_requests: 2,
            ..Policy::default()
        },
    );
    let r = run("r1");
    edit(&w, &r, "edited\n").await;
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    refused(&e, "open pull requests");
    assert!(!e.retryable, "{e:?}");
    nothing_pushed(&world);
    assert!(world.forge.opened.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_branch_with_a_pull_request_goes_on_at_the_cap() {
    let world = World::new();
    let w = held_to(
        &world,
        Policy {
            max_open_pull_requests: 1,
            ..Policy::default()
        },
    );
    let r = run("r1");
    edit(&w, &r, "first\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    edit(&w, &r, "second\n").await;
    w.propose(&r, &another_proposal()).await.unwrap();
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "second");
}

#[tokio::test]
async fn pushes_past_the_days_cap_are_refused_across_runs_and_restarts() {
    let world = World::new();
    let policy = Policy {
        max_pushes_per_day: 1,
        ..Policy::default()
    };
    let r = run("r1");
    let w = held_to(&world, policy.clone());
    edit(&w, &r, "first\n").await;
    let first = w.propose(&r, &proposal()).await.unwrap();
    drop(w);

    let w = held_to(&world, policy);
    edit(&w, &r, "second\n").await;
    let e = w.propose(&r, &another_proposal()).await.unwrap_err();
    refused(&e, "pushes");
    assert_eq!(Value::String(world.pushed(BRANCH)), first["commit"]);
}

#[tokio::test]
async fn a_refused_change_spends_nothing_of_the_days_pushes() {
    let world = World::new();
    let w = held_to(
        &world,
        Policy {
            max_pushes_per_day: 1,
            ..Policy::default()
        },
    );
    let r = run("r1");
    sh(&w, &r, "printf 'on: x\\n' > .github/ci.yml")
        .await
        .unwrap();
    w.propose(&r, &proposal()).await.unwrap_err();
    sh(
        &w,
        &r,
        "git reset --quiet --hard && printf 'edited\\n' > app.txt",
    )
    .await
    .unwrap();
    w.propose(&r, &another_proposal()).await.unwrap();
}

#[tokio::test]
async fn a_proposal_finished_after_a_failed_forge_is_not_counted_twice() {
    let world = World::new();
    let w = held_to(
        &world,
        Policy {
            max_pushes_per_day: 1,
            ..Policy::default()
        },
    );
    let r = run("r1");
    edit(&w, &r, "first\n").await;
    *world.forge.open_fails.lock().unwrap() = true;
    w.propose(&r, &proposal()).await.unwrap_err();
    *world.forge.open_fails.lock().unwrap() = false;
    w.propose(&r, &proposal()).await.unwrap();
}

#[tokio::test]
async fn a_forge_that_cannot_count_stops_a_change_before_it_leaves() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "edited\n").await;
    *world.forge.fails_with.lock().unwrap() = Some("down".to_string());
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(e.retryable);
    nothing_pushed(&world);
}

#[test]
fn a_target_without_follow_ups_says_so_in_its_offer() {
    let world = World::new();
    let offer = |policy: Policy| held_to(&world, policy).offer(&run("r1"), Some(QUEUE));
    assert!(!offer(Policy::default()).no_follow_ups);
    let quiet = Policy {
        follow_ups: false,
        ..Policy::default()
    };
    assert!(offer(quiet).no_follow_ups);
}

#[tokio::test]
async fn a_second_proposal_builds_on_the_first() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    let first = w.propose(&r, &proposal()).await.unwrap();
    edit(&w, &r, "second\n").await;
    let second = w.propose(&r, &another_proposal()).await.unwrap();
    assert_ne!(first["commit"], second["commit"]);
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "second");
    assert_eq!(
        Value::String(world.pushed(&format!("{BRANCH}~1"))),
        first["commit"]
    );
    assert_eq!(world.forge.opened.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_proposal_after_a_restart_builds_on_what_was_pushed() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    let first = w.propose(&r, &proposal()).await.unwrap();
    drop(w);

    let w = world.daemon();
    edit(&w, &r, "second\n").await;
    w.propose(&r, &another_proposal()).await.unwrap();
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "second");
    assert_eq!(
        Value::String(world.pushed(&format!("{BRANCH}~1"))),
        first["commit"]
    );
}

#[tokio::test]
async fn proposing_again_after_a_failed_forge_finishes_the_job() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    *world.forge.open_fails.lock().unwrap() = true;
    w.propose(&r, &proposal()).await.unwrap_err();
    *world.forge.open_fails.lock().unwrap() = false;
    let answer = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(answer["commit"], world.pushed(BRANCH));
    assert_eq!(world.forge.opened.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_commit_of_someone_elses_on_the_branch_is_not_undone_after_a_restart() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    drop(w);
    world.someone_pushes(BRANCH, BRANCH);
    let tip = world.pushed(BRANCH);

    let later = world.daemon().propose(&r, &another_proposal()).await;
    is_lost(&later.unwrap_err());
    assert_eq!(world.pushed(BRANCH), tip);
}

#[tokio::test]
async fn a_container_gone_altogether_is_lost_and_never_silently_remade() {
    let world = World::new();
    let r = run("r1");
    edit(&world.daemon(), &r, "edited\n").await;
    world.runtime.with(|b| b.live.clear());

    let w = world.daemon();
    is_lost(&sh(&w, &r, "cat app.txt").await.unwrap_err());
    is_lost(&sh(&w, &r, "cat app.txt").await.unwrap_err());
    is_lost(&w.propose(&r, &proposal()).await.unwrap_err());
    assert_eq!(world.runtime.made(), 1);
}

#[tokio::test]
async fn a_container_with_no_checkout_left_is_lost() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "edited\n").await;
    sh(&w, &r, &format!("rm -rf {REPO_DIR}/.git"))
        .await
        .unwrap();
    is_lost(&sh(&world.daemon(), &r, "true").await.unwrap_err());
}

#[tokio::test]
async fn a_crash_during_the_first_clone_starts_over_in_a_fresh_container() {
    let world = World::new();
    let spec = ContainerSpec {
        labels: BTreeMap::from([
            ("clax.run".to_string(), "r1".to_string()),
            ("clax.queue".to_string(), QUEUE.to_string()),
            (LABEL_BASE.to_string(), world.pushed("main")),
        ]),
        ..ContainerSpec::default()
    };
    let half = world.runtime.create(&spec).await.unwrap();
    let entry = Entry {
        container: Some(half.id().to_string()),
        labels: spec.labels.clone(),
        ..Entry::default()
    };
    Ledger::at(world.root.path().join("ledger"))
        .write("r1", &entry)
        .unwrap();
    let done = sh(&world.daemon(), &run("r1"), "cat app.txt")
        .await
        .unwrap();
    assert_eq!(done["exit_code"], 0, "{done}");
    assert_eq!(world.runtime.made(), 2, "a fresh one");
    assert_eq!(world.runtime.live(), 1, "and the half-made one is gone");
}

#[tokio::test]
async fn a_branch_whose_pull_request_is_over_is_finished_and_only_an_open_one_goes_on() {
    let world = World::new();
    let w = world.daemon();
    let first = run("r1");
    edit(&w, &first, "first\n").await;
    w.propose(&first, &proposal()).await.unwrap();
    w.finish(&first).await;
    // Merged or closed: `main` moved on, and no pull request is open.
    world.someone_pushes("main", "main");
    world.forge.opened.lock().unwrap().clear();

    let later = run("r2");
    let seen = sh(&w, &later, "cat theirs.txt app.txt").await.unwrap();
    let seen = seen["output"].as_str().unwrap();
    assert!(seen.contains("theirs") && seen.contains("one"), "{seen}");
    edit(&w, &later, "second\n").await;
    // A restart keeps the branch it started on.
    world.daemon().propose(&later, &proposal()).await.unwrap();
    let opened = world.forge.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0].head, "claxbot/thread1-2");
    assert_eq!(opened[0].base, "main");
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "first");
    assert_eq!(
        file_at(&world.bare, "claxbot/thread1-2", "app.txt"),
        "second"
    );
    assert_eq!(
        file_at(&world.bare, "claxbot/thread1-2", "theirs.txt"),
        "theirs"
    );
    w.finish(&later).await;

    let third = run("r3");
    let seen = sh(&w, &third, "cat app.txt").await.unwrap();
    assert_eq!(seen["output"], "second\n", "its pull request is open");
    edit(&w, &third, "third\n").await;
    w.propose(&third, &proposal()).await.unwrap();
    assert_eq!(world.forge.opened.lock().unwrap().len(), 1);
    assert_eq!(
        file_at(&world.bare, "claxbot/thread1-2", "app.txt"),
        "third"
    );
}

#[tokio::test]
async fn a_branch_number_nothing_can_follow_is_left_alone() {
    let world = World::new();
    world.someone_pushes("claxbot/thread1-4294967295", "main");
    let w = world.daemon();
    let r = run("r1");
    edit(&w, &r, "edited\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "edited");
}

#[tokio::test]
async fn a_runtime_that_cannot_be_asked_is_not_a_loss_and_makes_nothing() {
    let world = World::new();
    let r = run("r1");
    edit(&world.daemon(), &r, "edited\n").await;
    // With no ledger entry the listing is all there is to go on.
    std::fs::remove_dir_all(world.root.path().join("ledger")).unwrap();
    world.runtime.with(|b| b.find_fails = true);
    let w = world.daemon();
    let e = sh(&w, &r, "true").await.unwrap_err();
    assert!(e.retryable, "{}", e.message);
    assert_eq!(world.runtime.made(), 1);

    world.runtime.with(|b| b.find_fails = false);
    let answer = sh(&w, &r, "cat app.txt").await.unwrap();
    assert_eq!(answer["output"], "edited\n");
}

#[tokio::test]
async fn a_run_that_has_not_started_gets_a_container_and_a_proposal_is_refused() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
    sh(&w, &r, "true").await.unwrap();
    assert_eq!(world.runtime.made(), 1);
}

#[tokio::test]
async fn a_base_is_recorded_where_the_container_cannot_change_it() {
    let world = World::new();
    let r = run("r1");
    let main = world.pushed("main");
    sh(&world.daemon(), &r, "true").await.unwrap();
    let labels = world
        .runtime
        .with(|b| b.live.values().next().unwrap().1.clone());
    assert_eq!(labels[LABEL_BASE], main);
}

#[tokio::test]
async fn a_finished_run_after_a_restart_leaves_no_container() {
    let world = World::new();
    let r = run("r1");
    sh(&world.daemon(), &r, "true").await.unwrap();
    assert_eq!(world.runtime.live(), 1);
    let w = world.daemon();
    w.finish(&r).await;
    assert_eq!(world.runtime.live(), 0);
    w.finish(&r).await;
}

#[tokio::test]
async fn reap_ends_only_what_is_this_deployments_and_over() {
    let world = World::new();
    let w = world.daemon();
    let (orphan, active, held) = (run("orphan"), run("active"), run("held"));
    sh(&world.daemon(), &orphan, "true").await.unwrap();
    sh(&world.daemon(), &active, "true").await.unwrap();
    sh(&w, &held, "true").await.unwrap();
    // Another deployment's, and something with no run at all.
    let other = BTreeMap::from([
        ("clax.workspace".to_string(), "there".to_string()),
        ("clax.run".to_string(), "theirs".to_string()),
    ]);
    let no_run = BTreeMap::from([("clax.workspace".to_string(), "here".to_string())]);
    for labels in [other, no_run] {
        let spec = ContainerSpec {
            labels,
            ..ContainerSpec::default()
        };
        world.runtime.create(&spec).await.unwrap();
    }

    let keep = ["active".to_string()];
    assert_eq!(w.end_finished(&keep).await.unwrap(), 1);
    assert_eq!(world.runtime.live(), 4);
    let ledger = world.root.path().join("ledger");
    assert!(!ledger.join("orphan.json").exists());
    assert!(ledger.join("active.json").exists());
    assert!(ledger.join("held.json").exists());
}

/// A daemon of `world` whose target's preview is at `base` and is given
/// `reviewed` and ten minutes, remembering runs in the world's ledger.
fn keeping_watch(world: &World, base: &str, reviewed: bool) -> ContainerWorkspace {
    let mut t = world.target();
    t.preview.url_template = format!("{base}/{{pr}}");
    t.preview.ready_names_commit = true;
    t.preview.timeout = Duration::from_secs(600);
    t.preview.poll = Duration::from_millis(5);
    t.reviewed_previews = reviewed;
    ContainerWorkspace::keeping(
        Arc::new(world.runtime.clone()),
        world.daemon().spec.clone(),
        vec![t],
        true,
        world.root.path().join("ledger"),
        world.secrets.clone(),
    )
}

#[tokio::test]
async fn reap_keeps_a_finished_run_while_its_preview_is_waited_for_and_no_longer() {
    let world = World::new();
    let body = Arc::new(Mutex::new(String::new()));
    let base = preview_server(body.clone()).await;
    let first = keeping_watch(&world, &base, false);
    let r = run("r1");
    edit(&first, &r, "edited\n").await;
    let answer = first.propose(&r, &proposal()).await.unwrap();
    first.finish(&r).await;
    drop(first);

    // A restart in the wait reaps it; the next one still takes up the wait.
    keeping_watch(&world, &base, false)
        .end_finished(&[])
        .await
        .unwrap();
    let told = Arc::new(Told::default());
    keeping_watch(&world, &base, false).previews(told.clone());
    *body.lock().unwrap() = answer["commit"].as_str().unwrap().to_string();
    told.heard().await;
    assert_eq!(told.ready.lock().unwrap()[0].0, "r1");

    let ledger = Ledger::at(world.root.path().join("ledger"));
    let mut entry = ledger.read("r1").unwrap();
    entry.awaiting.as_mut().unwrap().until = 1;
    ledger.write("r1", &entry).unwrap();
    world.daemon().end_finished(&[]).await.unwrap();
    assert_eq!(
        ledger.read("r1").unwrap(),
        Entry::default(),
        "its wait is over"
    );
}

#[tokio::test]
async fn a_step_run_again_after_a_crash_hears_of_its_preview_again() {
    let world = World::new();
    let body = Arc::new(Mutex::new(String::new()));
    let base = preview_server(body.clone()).await;
    let first = keeping_watch(&world, &base, false);
    let r = run("r1");
    edit(&first, &r, "edited\n").await;
    let answer = first.propose(&r, &proposal()).await.unwrap();
    drop(first);
    *body.lock().unwrap() = answer["commit"].as_str().unwrap().to_string();

    // The daemon comes back and the news goes out before the fix is on the
    // report, which drops it; then the step runs again.
    let told = Arc::new(Told::default());
    let w = keeping_watch(&world, &base, false);
    w.previews(told.clone());
    told.heard().await;
    assert_eq!(w.propose(&r, &proposal()).await.unwrap(), answer);
    told.heard().await;
    assert_eq!(told.ready.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_step_run_again_past_its_wait_hears_that_the_wait_ended() {
    let world = World::new();
    let base = preview_server(Arc::default()).await;
    let first = keeping_watch(&world, &base, false);
    let r = run("r1");
    edit(&first, &r, "edited\n").await;
    let answer = first.propose(&r, &proposal()).await.unwrap();
    drop(first);
    let ledger = Ledger::at(world.root.path().join("ledger"));
    let mut entry = ledger.read("r1").unwrap();
    entry.awaiting.as_mut().unwrap().until = 1;
    ledger.write("r1", &entry).unwrap();

    let told = Arc::new(Told::default());
    let w = keeping_watch(&world, &base, false);
    w.previews(told.clone());
    told.heard().await;
    assert_eq!(w.propose(&r, &proposal()).await.unwrap(), answer);
    told.heard().await;
    assert_eq!(*told.missed.lock().unwrap(), ["r1", "r1"]);
}

#[tokio::test]
async fn a_preview_is_told_by_what_the_run_was_offered_whatever_its_target_says_now() {
    for (offered, now) in [(false, true), (true, false)] {
        let world = World::new();
        let body = Arc::new(Mutex::new(String::new()));
        let base = preview_server(body.clone()).await;
        let first = keeping_watch(&world, &base, offered);
        let r = run("r1");
        edit(&first, &r, "edited\n").await;
        let answer = first.propose(&r, &proposal()).await.unwrap();
        first.finish(&r).await;
        drop(first);

        let told = Arc::new(Told::default());
        keeping_watch(&world, &base, now).previews(told.clone());
        *body.lock().unwrap() = answer["commit"].as_str().unwrap().to_string();
        told.heard().await;
        assert_eq!(*told.reviewed.lock().unwrap(), [offered]);
    }
}

#[tokio::test]
async fn a_proposal_after_a_restart_tells_its_preview_by_what_the_run_was_offered() {
    let world = World::new();
    let body = Arc::new(Mutex::new(String::new()));
    let base = preview_server(body.clone()).await;
    let first = keeping_watch(&world, &base, false);
    let r = run("r1");
    edit(&first, &r, "edited\n").await;
    drop(first);

    let told = Arc::new(Told::default());
    let w = keeping_watch(&world, &base, true);
    w.previews(told.clone());
    let answer = w.propose(&r, &proposal()).await.unwrap();
    *body.lock().unwrap() = answer["commit"].as_str().unwrap().to_string();
    told.heard().await;
    assert_eq!(*told.reviewed.lock().unwrap(), [false]);
}

#[tokio::test]
async fn reap_refuses_a_workspace_with_no_labels_to_tell_its_own() {
    let world = World::new();
    let w = ContainerWorkspace::new(
        Arc::new(world.runtime.clone()),
        ContainerSpec::default(),
        Vec::new(),
        true,
    );
    assert!(w.end_finished(&[]).await.is_err());
}

fn answering(output: &str, exit_code: i32) -> ExecOutput {
    ExecOutput {
        exit_code,
        output: output.to_string(),
    }
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Runs `run` to a proposal with the container answering `lie`, and fails
/// unless nothing reached the remote.
async fn lied_to(lie: ExecOutput) -> HostError {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    sh(&w, &r, "true").await.unwrap();
    world.runtime.with(|b| b.patch_answer = Some(lie));
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert!(
        !git(&world.bare, &["branch", "--list"]).contains("claxbot"),
        "something was pushed"
    );
    assert!(world.forge.opened.lock().unwrap().is_empty());
    e
}

#[tokio::test]
async fn a_container_that_says_it_succeeded_with_garbage_gets_nowhere() {
    let e = lied_to(answering("!!! not base64 !!!", 0)).await;
    assert!(e.message.contains("not a change"), "{}", e.message);
    let e = lied_to(answering(&base64_of(b"not a patch\n"), 0)).await;
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
}

#[tokio::test]
async fn a_container_that_says_it_failed_gets_nowhere_whatever_it_sent() {
    let good = "diff --git a/app.txt b/app.txt\n--- a/app.txt\n+++ b/app.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n";
    let e = lied_to(answering(&base64_of(good.as_bytes()), 1)).await;
    assert!(e.message.contains("could not be read"), "{}", e.message);
}

#[tokio::test]
async fn a_hostile_patch_from_a_container_that_says_it_succeeded_is_refused() {
    let hook = "diff --git a/.git/hooks/pre-push b/.git/hooks/pre-push\nnew file mode 100755\n--- /dev/null\n+++ b/.git/hooks/pre-push\n@@ -0,0 +1 @@\n+touch /tmp/owned\n";
    let e = lied_to(answering(&base64_of(hook.as_bytes()), 0)).await;
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
}

#[tokio::test]
async fn output_over_the_limit_is_refused_and_says_so() {
    let e = lied_to(answering(&"A".repeat(5 * 1024 * 1024), 0)).await;
    assert!(e.message.contains("size limit"), "{}", e.message);
    // What fails with a flood is clipped, not repeated in full.
    let e = lied_to(answering(&"x".repeat(1024 * 1024), 1)).await;
    assert!(e.message.len() < 4096, "{}", e.message.len());
}

#[tokio::test]
async fn what_git_warns_on_stderr_does_not_corrupt_the_patch() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    // `git add` warns about a repository inside the checkout.
    let made = sh(
        &w,
        &r,
        "git init --quiet nested && git -C nested -c user.name=T -c user.email=t@e commit \
         --quiet --allow-empty -m x",
    )
    .await
    .unwrap();
    assert_eq!(made["exit_code"], 0, "{made}");
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
    assert!(e.message.contains("submodule"), "{}", e.message);
}

#[tokio::test]
async fn a_host_clone_that_fails_makes_no_container() {
    let mut world = World::new();
    world.url = "file:///nowhere/at/all.git".to_string();
    let w = world.daemon();
    let e = sh(&w, &run("r1"), "true").await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert_eq!(world.runtime.made(), 0, "the host clones first");
}

const WRITE: &str = "wr1te-token-value";
const READ: &str = "r3ad-token-value";

/// A world whose container clones from a remote that demands `READ`, given
/// `container_token`. The host has a remote of its own, demanding `WRITE`.
async fn authenticated(container_token: &str) -> (World, crate::fixture::Remote) {
    let mut world = World::new();
    let containers = serve(world.root.path(), READ, None).await;
    world.url = containers.url();
    world.write_token = Some(Token::new(WRITE));
    world.read_token = Some(Token::new(container_token));
    (world, containers)
}

/// As [`World::daemon`], with the two remotes of an authenticated world.
fn daemon_of(world: &World, host_url: &str) -> ContainerWorkspace {
    let mut t = world.target();
    t.repo_url = host_url.to_string();
    ContainerWorkspace::keeping(
        Arc::new(world.runtime.clone()),
        ContainerSpec::default(),
        vec![t],
        false,
        world.root.path().join("ledger"),
        world.secrets.clone(),
    )
}

fn no_token_in(text: &str) {
    for token in [READ, WRITE, "wr0ng-read-token"] {
        assert!(!text.contains(token), "a token leaked: {text}");
        let header = basic(token);
        let encoded = header.strip_prefix("Basic ").unwrap();
        assert!(!text.contains(encoded), "a token leaked: {text}");
    }
}

#[tokio::test]
async fn a_private_repository_reaches_a_container_with_the_read_token_alone() {
    let (world, containers) = authenticated(READ).await;
    let host = serve(world.root.path(), WRITE, None).await;
    let w = daemon_of(&world, &host.url());
    let r = run("r1");
    let answer = sh(&w, &r, "cat app.txt").await.unwrap();
    assert_eq!(answer["output"], "one\ntwo\nthree\n");
    assert!(
        containers
            .seen
            .lock()
            .unwrap()
            .authorizations
            .iter()
            .all(|a| *a == basic(READ))
    );

    // The write token never crossed, and the read token crossed once.
    let execs = world.runtime.with(|b| b.execs.clone());
    for e in &execs {
        for value in e.envs.values() {
            assert!(!value.contains(&basic(WRITE)));
        }
    }
    let carrying = execs
        .iter()
        .filter(|e| e.envs.contains_key("GIT_CONFIG_VALUE_0"))
        .count();
    assert_eq!(carrying, 1);
    let config = sh(&w, &r, "cat .git/config").await.unwrap();
    no_token_in(config["output"].as_str().unwrap());
}

#[tokio::test]
async fn a_wrong_read_token_fails_the_run_clearly_with_no_token_in_the_failure() {
    let (world, _containers) = authenticated("wr0ng-read-token").await;
    let host = serve(world.root.path(), WRITE, None).await;
    let w = daemon_of(&world, &host.url());
    let e = sh(&w, &run("r1"), "true").await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(!e.retryable, "asking again will not help");
    assert!(e.message.contains("refused"), "{}", e.message);
    assert!(e.message.contains("credentials"), "{}", e.message);
    assert!(!e.message.contains("Try again"), "{}", e.message);
    no_token_in(&e.message);
    assert_eq!(world.runtime.live(), 0, "its container is gone");
}

#[tokio::test]
async fn a_wrong_write_token_fails_the_run_clearly_too() {
    let (world, _containers) = authenticated(READ).await;
    let host = serve(world.root.path(), "another-write-token", None).await;
    let w = daemon_of(&world, &host.url());
    let e = sh(&w, &run("r1"), "true").await.unwrap_err();
    assert!(!e.retryable);
    assert!(e.message.contains("credentials"), "{}", e.message);
    no_token_in(&e.message);
    assert_eq!(world.runtime.made(), 0);
}

#[tokio::test]
async fn what_a_remote_echoes_to_a_containers_clone_holds_no_token() {
    let (mut world, containers) = authenticated(READ).await;
    let host = serve(world.root.path(), WRITE, None).await;
    world.url = format!("{}/echo/repo.git", containers.base);
    let w = daemon_of(&world, &host.url());
    let e = sh(&w, &run("r1"), "true").await.unwrap_err();
    no_token_in(&e.message);
}

#[tokio::test]
async fn a_forges_error_and_a_commands_output_hold_no_token() {
    let (world, _containers) = authenticated(READ).await;
    let host = serve(world.root.path(), WRITE, None).await;
    let w = daemon_of(&world, &host.url());
    let r = run("r1");
    sh(&w, &r, "printf 'changed\\n' > app.txt").await.unwrap();
    *world.forge.fails_with.lock().unwrap() = Some(format!("bad credentials {WRITE}"));
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert!(!e.message.contains("bad credentials"), "{}", e.message);
    no_token_in(&e.message);
    let answer = sh(&w, &r, &format!("echo {READ}")).await.unwrap();
    no_token_in(answer["output"].as_str().unwrap());
}

/// The commands above run on this machine's shell; these run on a real
/// container's, through a restart. `CLAX_TEST_DOCKER` names an image with git
/// that trusts a mounted repository (`demo/claxbot-workspace/sandbox.Dockerfile`).
#[tokio::test]
async fn a_real_container_survives_a_restart_and_its_change_is_pushed() {
    let Ok(image) = std::env::var("CLAX_TEST_DOCKER_IMAGE") else {
        crate::skipped("a Docker workspace test: CLAX_TEST_DOCKER_IMAGE is not set");
        return;
    };
    let world = World::new();
    let bare = world.bare.canonicalize().unwrap();
    let daemon = || {
        let mut t = world.target();
        t.container_repo_url = "/srv/repo.git".to_string();
        let docker = clax_bot_workspace::docker::Docker {
            run_args: vec![
                "--volume".to_string(),
                format!("{}:/srv/repo.git:ro", bare.display()),
            ],
        };
        let spec = ContainerSpec {
            image: Some(image.clone()),
            labels: BTreeMap::from([("clax.workspace".to_string(), "test".to_string())]),
            auto_stop_minutes: Some(5),
            ..ContainerSpec::default()
        };
        ContainerWorkspace::keeping(
            Arc::new(docker),
            spec,
            vec![t],
            false,
            world.root.path().join("ledger"),
            world.secrets.clone(),
        )
    };
    let r = run(&format!("r{}", std::process::id()));
    let binary = "head -c 4096 /dev/urandom > blob.bin";
    edit(&daemon(), &r, "edited\n").await;
    let w = daemon();
    assert_eq!(sh(&w, &r, binary).await.unwrap()["exit_code"], 0);
    let answer = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(answer["commit"], world.pushed(BRANCH));
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "edited");
    let size = git(
        &world.bare,
        &["cat-file", "-s", &format!("{BRANCH}:blob.bin")],
    );
    assert_eq!(size, "4096");
    w.finish(&r).await;
    let left = daemon().end_finished(&[]).await.unwrap();
    assert_eq!(left, 0, "finish left nothing to reap");
}

#[tokio::test]
async fn a_preview_that_never_answers_does_not_hold_up_a_proposal() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Accepts and says nothing.
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            held.push(listener.accept().await.unwrap());
        }
    });
    let world = World::new();
    let mut t = world.target();
    t.preview.url_template = format!("http://{addr}/{{pr}}");
    t.preview.timeout = Duration::from_secs(3600);
    let spec = world.daemon().spec.clone();
    let w = ContainerWorkspace::new(Arc::new(world.runtime.clone()), spec, vec![t], true);
    let r = run("r1");
    edit(&w, &r, "edited\n").await;
    let answered = tokio::time::timeout(Duration::from_secs(30), w.propose(&r, &proposal())).await;
    let answer = answered.expect("propose waited on the preview").unwrap();
    assert_eq!(answer["verify_url"], Value::Null);
}

#[tokio::test]
async fn a_container_that_stops_under_a_run_comes_back_at_the_next_call() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "edited\n").await;
    world.runtime.with(|b| b.stopped = true);
    let e = sh(&w, &r, "echo once >> ran").await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    let answer = sh(&w, &r, "cat app.txt; cat ran").await.unwrap();
    assert!(answer["output"].as_str().unwrap().contains("edited"));
    assert!(!answer["output"].as_str().unwrap().contains("once"));
}

#[tokio::test]
async fn no_runtime_text_reaches_a_run() {
    let world = World::new();
    let r = run("r1");
    edit(&world.daemon(), &r, "edited\n").await;
    world.runtime.with(|b| b.attach_fails = true);
    let e = sh(&world.daemon(), &r, "true").await.unwrap_err();
    assert!(!e.message.contains("box-secret-detail"), "{}", e.message);
    assert!(e.retryable, "asking again may work");
}

#[tokio::test]
async fn an_attach_that_could_not_ask_is_not_a_loss() {
    let world = World::new();
    let r = run("r1");
    edit(&world.daemon(), &r, "edited\n").await;
    world.runtime.with(|b| b.attach_fails = true);
    let w = world.daemon();
    let e = sh(&w, &r, "true").await.unwrap_err();
    assert!(!e.message.contains("workspace was lost"), "{}", e.message);
    world.runtime.with(|b| b.attach_fails = false);
    assert_eq!(
        sh(&w, &r, "cat app.txt").await.unwrap()["output"],
        "edited\n"
    );
}

#[tokio::test]
async fn a_known_container_is_attached_without_listing() {
    let world = World::new();
    let r = run("r1");
    edit(&world.daemon(), &r, "edited\n").await;
    world.runtime.with(|b| b.find_fails = true);
    let answer = sh(&world.daemon(), &r, "cat app.txt").await.unwrap();
    assert_eq!(answer["output"], "edited\n");
}

#[derive(Default)]
struct Told {
    ready: Mutex<Vec<(String, String)>>,
    /// What each of `ready` said of the run's offer.
    reviewed: Mutex<Vec<bool>>,
    missed: Mutex<Vec<String>>,
    arrived: tokio::sync::Notify,
}

impl Told {
    /// Waits to be told, a hang guard's time at most.
    async fn heard(&self) {
        let hang = Duration::from_secs(60);
        tokio::time::timeout(hang, self.arrived.notified())
            .await
            .expect("the sink was told");
    }
}

#[async_trait::async_trait]
impl PreviewSink for Told {
    async fn preview_ready(
        &self,
        run: &str,
        verify_url: &str,
        reviewed: bool,
    ) -> Result<(), HostError> {
        let told = (run.to_string(), verify_url.to_string());
        self.ready.lock().unwrap().push(told);
        self.reviewed.lock().unwrap().push(reviewed);
        self.arrived.notify_one();
        Ok(())
    }

    async fn preview_missed(&self, run: &str) -> Result<(), HostError> {
        self.missed.lock().unwrap().push(run.to_string());
        self.arrived.notify_one();
        Ok(())
    }
}

/// A preview server: `/{pr}` answers what `body` holds.
async fn preview_server(body: Arc<Mutex<String>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(move || {
        let body = body.clone();
        async move { body.lock().unwrap().clone() }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

/// A workspace whose target's preview is at `base`, telling `told`.
fn watching(world: &World, base: &str, told: &Arc<Told>) -> ContainerWorkspace {
    let mut t = world.target();
    t.preview.url_template = format!("{base}/{{pr}}");
    t.preview.ready_names_commit = true;
    t.preview.timeout = Duration::from_secs(600);
    t.preview.poll = Duration::from_millis(5);
    let spec = world.daemon().spec.clone();
    let w = ContainerWorkspace::new(Arc::new(world.runtime.clone()), spec, vec![t], true);
    w.previews(told.clone());
    w
}

#[tokio::test]
async fn a_preview_is_told_of_after_the_proposal_and_after_the_run() {
    let world = World::new();
    let body = Arc::new(Mutex::new(String::new()));
    let base = preview_server(body.clone()).await;
    let told = Arc::new(Told::default());
    let w = watching(&world, &base, &told);
    let r = run("r1");
    edit(&w, &r, "edited\n").await;
    let answer = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(answer["verify_url"], Value::Null);
    w.finish(&r).await;
    *body.lock().unwrap() = answer["commit"].as_str().unwrap().to_string();
    told.arrived.notified().await;
    let ready = told.ready.lock().unwrap().clone();
    assert_eq!(ready, vec![("r1".to_string(), format!("{base}/1"))]);
}

#[tokio::test]
async fn a_wait_that_ends_with_no_preview_is_told() {
    let world = World::new();
    let told = Arc::new(Told::default());
    let w = world.daemon();
    w.previews(told.clone());
    let r = run("r1");
    edit(&w, &r, "edited\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    told.heard().await;
    assert_eq!(*told.missed.lock().unwrap(), ["r1"]);
    assert!(told.ready.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_wait_whose_end_a_restart_passed_is_told_when_the_daemon_comes_back() {
    let world = World::new();
    let entry = Entry {
        labels: BTreeMap::from([("clax.queue".to_string(), QUEUE.to_string())]),
        awaiting: Some(Awaiting {
            pr: 1,
            commit: "abc".to_string(),
            until: 1,
        }),
        ..Entry::default()
    };
    Ledger::at(world.root.path().join("ledger"))
        .write("r1", &entry)
        .unwrap();
    let told = Arc::new(Told::default());
    world.daemon().previews(told.clone());
    told.heard().await;
    assert_eq!(*told.missed.lock().unwrap(), ["r1"]);
}

#[tokio::test]
async fn a_wait_whose_end_a_restart_passed_looks_once_more_before_it_says_it_saw_none() {
    let world = World::new();
    let body = Arc::new(Mutex::new("abc".to_string()));
    let base = preview_server(body).await;
    let entry = Entry {
        labels: BTreeMap::from([("clax.queue".to_string(), QUEUE.to_string())]),
        awaiting: Some(Awaiting {
            pr: 1,
            commit: "abc".to_string(),
            until: 1,
        }),
        ..Entry::default()
    };
    Ledger::at(world.root.path().join("ledger"))
        .write("r1", &entry)
        .unwrap();
    let told = Arc::new(Told::default());
    keeping_watch(&world, &base, false).previews(told.clone());
    told.heard().await;
    assert_eq!(told.ready.lock().unwrap()[0].0, "r1");
    assert!(told.missed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_second_proposal_supersedes_the_wait_for_the_first() {
    let world = World::new();
    let body = Arc::new(Mutex::new(String::new()));
    let base = preview_server(body.clone()).await;
    let told = Arc::new(Told::default());
    let w = watching(&world, &base, &told);
    let r = run("r1");
    edit(&w, &r, "first\n").await;
    let first = w.propose(&r, &proposal()).await.unwrap();
    edit(&w, &r, "second\n").await;
    let second = w.propose(&r, &another_proposal()).await.unwrap();
    // Serving both commits, only the second's wait is left to see it.
    let both = format!("{} {}", first["commit"], second["commit"]);
    *body.lock().unwrap() = both;
    told.arrived.notified().await;
    assert_eq!(told.ready.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_key_seen_before_answers_as_it_did_and_pushes_nothing() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    let first = w.propose(&r, &proposal()).await.unwrap();
    edit(&w, &r, "second\n").await;
    let again = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(again, first);
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "first");

    let restarted = world.daemon().propose(&r, &proposal()).await.unwrap();
    assert_eq!(restarted, first);
}

#[tokio::test]
async fn a_crash_after_the_push_finds_the_pull_request_and_pushes_nothing_more() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "first\n").await;
    w.propose(&r, &proposal()).await.unwrap();
    let tip = world.pushed(BRANCH);
    // As if the daemon died between the push and writing down its answer.
    let path = world.root.path().join("ledger/r1.json");
    let mut entry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    entry["proposals"] = serde_json::json!({});
    entry["pending"] = serde_json::json!({"key": "k", "commit": tip});
    entry["pushed"] = serde_json::json!([]);
    std::fs::write(&path, entry.to_string()).unwrap();
    edit(&w, &r, "moved on\n").await;

    let answer = world.daemon().propose(&r, &proposal()).await.unwrap();
    assert_eq!(answer["commit"], tip);
    assert_eq!(answer["pr"], 1);
    assert_eq!(world.pushed(BRANCH), tip);
    assert_eq!(world.forge.opened.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_finished_run_gets_no_new_container() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    sh(&w, &r, "true").await.unwrap();
    w.finish(&r).await;
    let e = sh(&w, &r, "true").await.unwrap_err();
    assert!(e.message.contains("ended"), "{}", e.message);
    let e = sh(&world.daemon(), &r, "true").await.unwrap_err();
    assert!(e.message.contains("ended"), "{}", e.message);
    assert_eq!(world.runtime.made(), 1);
}

#[tokio::test]
async fn finish_ends_a_container_no_ledger_knew_of() {
    let world = World::new();
    let r = run("r1");
    sh(&world.daemon(), &r, "true").await.unwrap();
    std::fs::remove_dir_all(world.root.path().join("ledger")).unwrap();
    world.daemon().finish(&r).await;
    assert_eq!(world.runtime.live(), 0);
}

#[tokio::test]
async fn a_run_on_another_queue_does_not_take_a_container_as_its_own() {
    let world = World::new();
    let r = run("r1");
    sh(&world.daemon(), &r, "true").await.unwrap();
    let e = world
        .daemon()
        .exec(&r, None, "true", None, 30)
        .await
        .unwrap_err();
    is_lost(&e);
}

/// Hands out a new token each time, as a short-lived source does.
#[derive(Default)]
struct Rotating {
    issued: Mutex<Vec<String>>,
}

impl Rotating {
    fn next(&self) -> Token {
        let mut issued = self.issued.lock().unwrap();
        let t = format!("rotating-token-{}", issued.len());
        issued.push(t.clone());
        Token::new(t)
    }
}

#[async_trait::async_trait]
impl RepoAccess for Rotating {
    async fn read_token(&self) -> Result<Option<Token>> {
        Ok(Some(self.next()))
    }
    async fn write_token(&self) -> Result<Option<Token>> {
        Ok(Some(self.next()))
    }
}

#[tokio::test]
async fn tokens_are_asked_for_at_each_use_and_redacted_after_it() {
    let world = World::new();
    let access = Arc::new(Rotating::default());
    let mut t = world.target();
    t.access = access.clone();
    let spec = world.daemon().spec.clone();
    let dir = world.root.path().join("ledger");
    let secrets = world.secrets.clone();
    let runtime = Arc::new(world.runtime.clone());
    let w = ContainerWorkspace::keeping(runtime, spec, vec![t], true, dir, secrets);
    let r = run("r1");
    edit(&w, &r, "first\n").await;
    // One for the host's clone, one for the container's.
    assert_eq!(access.issued.lock().unwrap().len(), 2);
    w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(access.issued.lock().unwrap().len(), 3, "one for the push");
    let issued = access.issued.lock().unwrap().clone();
    for token in &issued {
        let answer = sh(&w, &r, &format!("echo {token}")).await.unwrap();
        assert_eq!(answer["output"], "[redacted]\n");
    }
}

#[tokio::test]
async fn a_confidential_target_makes_a_confidential_offer() {
    let world = World::new();
    let mut t = world.target();
    t.confidential = true;
    let spec = world.daemon().spec.clone();
    let open = world.daemon();
    let closed = ContainerWorkspace::new(Arc::new(world.runtime.clone()), spec, vec![t], true);
    let r = run("r1");
    assert!(closed.offer(&r, Some(QUEUE)).confidential);
    assert!(closed.offer(&r, Some(QUEUE)).change);
    assert!(!open.offer(&r, Some(QUEUE)).confidential);
    assert!(!closed.offer(&r, None).confidential);
    assert!(!closed.offer(&r, Some(QUEUE)).reviewed_previews);
}

#[tokio::test]
async fn a_target_whose_deploys_are_reviewed_says_so() {
    let world = World::new();
    let mut t = world.target();
    t.reviewed_previews = true;
    let w = ContainerWorkspace::new(
        Arc::new(world.runtime.clone()),
        ContainerSpec::default(),
        vec![t],
        true,
    );
    assert!(w.offer(&run("r1"), Some(QUEUE)).reviewed_previews);
}

#[tokio::test]
async fn a_real_container_clones_from_a_remote_that_demands_the_read_token() {
    let Ok(image) = std::env::var("CLAX_TEST_DOCKER_IMAGE") else {
        crate::skipped("a Docker clone test: CLAX_TEST_DOCKER_IMAGE is not set");
        return;
    };
    let world = World::new();
    let remote = crate::fixture::serve_on("0.0.0.0", world.root.path(), READ, None).await;
    let port = remote.base.rsplit(':').next().unwrap();
    let docker = clax_bot_workspace::docker::Docker {
        run_args: vec![
            "--add-host".to_string(),
            "host.docker.internal:host-gateway".to_string(),
        ],
    };
    let mut t = world.target();
    t.container_repo_url = format!("http://host.docker.internal:{port}/repo.git");
    t.access = Arc::new(StaticAccess {
        read: Some(Token::new(READ)),
        write: None,
    });
    let spec = ContainerSpec {
        image: Some(image),
        auto_stop_minutes: Some(5),
        ..ContainerSpec::default()
    };
    let w = ContainerWorkspace::new(Arc::new(docker), spec, vec![t], false);
    let r = run(&format!("clone{}", std::process::id()));
    let answer = sh(&w, &r, "cat app.txt; cat .git/config").await;
    w.finish(&r).await;
    let answer = answer.unwrap();
    let output = answer["output"].as_str().unwrap();
    assert!(output.contains("three"), "{output}");
    no_token_in(output);
}

fn hostile(url_template: String) -> Preview {
    Preview {
        url_template,
        ready_path: String::new(),
        ready_names_commit: true,
        timeout: Duration::ZERO,
        poll: Duration::ZERO,
    }
}

/// Whether `preview` reads as up, a test failing rather than hanging if the
/// look does not end.
async fn looks_up(preview: &Preview, commit: &str) -> bool {
    let http = clax_bot_workspace::runtime::control_client();
    let look = preview.wait(&http, 1, commit);
    tokio::time::timeout(Duration::from_secs(60), look)
        .await
        .expect("a look at a preview ended")
}

#[tokio::test]
async fn a_preview_that_never_answers_is_given_up_on() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            held.push(listener.accept().await.unwrap());
        }
    });
    assert!(!looks_up(&hostile(format!("http://{addr}/{{pr}}")), "abc").await);
}

#[tokio::test]
async fn a_commit_past_the_read_limit_of_a_previews_answer_is_not_seen() {
    let body = Arc::new(Mutex::new(format!("{}abc", "x".repeat(1 << 20))));
    let base = preview_server(body.clone()).await;
    let p = hostile(format!("{base}/{{pr}}"));
    assert!(!looks_up(&p, "abc").await);
    *body.lock().unwrap() = "abc".to_string();
    assert!(looks_up(&p, "abc").await);
}

#[tokio::test]
async fn a_preview_that_redirects_is_not_followed() {
    let body = Arc::new(Mutex::new("abc".to_string()));
    let base = preview_server(body).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(move || {
        let to = base.clone();
        async move { axum::response::Redirect::temporary(&to) }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    assert!(!looks_up(&hostile(format!("http://{addr}/{{pr}}")), "abc").await);
}

#[tokio::test]
async fn a_proposal_is_what_finds_a_stopped_container_and_it_comes_back() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "edited\n").await;
    world.runtime.with(|b| b.stopped = true);
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    let answer = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(answer["commit"], world.pushed(BRANCH));
    assert_eq!(file_at(&world.bare, BRANCH, "app.txt"), "edited");
}

#[tokio::test]
async fn a_push_that_failed_is_not_answered_as_done_by_the_same_key() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "edited\n").await;
    let aside = world.bare.with_extension("aside");
    std::fs::rename(&world.bare, &aside).unwrap();
    w.propose(&r, &proposal()).await.unwrap_err();
    std::fs::rename(&aside, &world.bare).unwrap();

    let answer = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(answer["commit"], world.pushed(BRANCH));
}

#[tokio::test]
async fn a_failure_after_the_push_does_not_say_nothing_changed() {
    let world = World::new();
    let r = run("r1");
    let w = world.daemon();
    edit(&w, &r, "edited\n").await;
    *world.forge.open_fails.lock().unwrap() = true;
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert!(e.message.contains("was pushed"), "{}", e.message);
    assert!(e.retryable);
}

#[tokio::test]
async fn a_token_in_what_the_model_wrote_does_not_reach_a_pull_request() {
    let (world, _containers) = authenticated(READ).await;
    let host = serve(world.root.path(), WRITE, None).await;
    let w = daemon_of(&world, &host.url());
    let r = run("r1");
    sh(&w, &r, "printf 'changed\\n' > app.txt").await.unwrap();
    let leaky = Proposal {
        title: format!("fix {READ}"),
        summary: format!("used {READ}"),
        key: "k".to_string(),
    };
    w.propose(&r, &leaky).await.unwrap();
    let opened = world.forge.opened.lock().unwrap();
    no_token_in(&opened[0].title);
    no_token_in(&opened[0].body);
}

#[tokio::test]
async fn a_restart_takes_up_the_wait_for_a_preview() {
    let world = World::new();
    let body = Arc::new(Mutex::new(String::new()));
    let base = preview_server(body.clone()).await;
    let target = || {
        let mut t = world.target();
        t.preview.url_template = format!("{base}/{{pr}}");
        t.preview.ready_names_commit = true;
        t.preview.timeout = Duration::from_secs(600);
        t.preview.poll = Duration::from_millis(5);
        t
    };
    let daemon = || {
        ContainerWorkspace::keeping(
            Arc::new(world.runtime.clone()),
            world.daemon().spec.clone(),
            vec![target()],
            true,
            world.root.path().join("ledger"),
            world.secrets.clone(),
        )
    };
    let r = run("r1");
    let first = daemon();
    edit(&first, &r, "edited\n").await;
    let answer = first.propose(&r, &proposal()).await.unwrap();
    drop(first);

    let told = Arc::new(Told::default());
    daemon().previews(told.clone());
    *body.lock().unwrap() = answer["commit"].as_str().unwrap().to_string();
    told.arrived.notified().await;
    assert_eq!(told.ready.lock().unwrap()[0].0, "r1");
}

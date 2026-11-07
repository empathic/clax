//! A repository a queue's admin connected in Clax, driving a workspace: a real
//! store and a real `git`, the store's link looked up per run.

use crate::fixture::{Remote, basic, seed, serve};
use crate::github::{self, Fake};
use crate::recovery::{Local, World, another_proposal, file_at, proposal, run};
use axum::Router;
use axum::extract::{Request, State};
use axum::http::Method;
use axum::response::{IntoResponse, Response};
use clax_bot::workspace::{CODE_CHANGE_REFUSED, CODE_SHELL_UNAVAILABLE, Offer, Workspace};
use clax_bot_workspace::linked::{Described, StoreTargets, TargetSource};
use clax_bot_workspace::net::{Names, PublicOnly};
use clax_bot_workspace::repo::{Clone, Token};
use clax_bot_workspace::runtime::ContainerSpec;
use clax_bot_workspace::workspace::{ContainerWorkspace, Target};
use clax_core::Home;
use clax_core::audit::AuditCtx;
use clax_core::queue::ReportersSee;
use clax_store::Store;
use clax_store::bot_settings::BaseUrlPolicy;
use clax_store::queue_repos::{
    Egress, GithubAppFields, LinkStatus, PUBLISH_FAILED, RepoAccess, RepoLinkFields, RepoUse,
    TokenKind,
};
use clax_store::queues::NewQueue;
use clax_store::secrets::Kek;
use serde_json::{Value, json};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

const CTX: &AuditCtx = &AuditCtx::DAEMON;
const READ: &str = "github_pat_READ_0123456789-read";
const WRITE: &str = "github_pat_WRITE_0123456789-rite";

fn kek() -> Kek {
    Kek::from_bytes([7; 32])
}

/// A store with one queue.
fn store(policy: BaseUrlPolicy) -> (TempDir, Arc<Store>, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&Home::at(dir.path().join("home"))).unwrap();
    store.set_base_url_policy(policy);
    let queue = NewQueue {
        name: "App".into(),
        origins: vec![],
        reporters_see: ReportersSee::Own,
    };
    let qid = store.create_queue(CTX, None, queue).unwrap().id;
    (dir, Arc::new(store), qid)
}

/// A link of `extra`'s fields on top of a token link to `repo_url`.
fn fields(repo_url: &str, forge: &str, extra: &Value) -> RepoLinkFields {
    let mut v = json!({
        "repo": "o/r",
        "repo_url": repo_url,
        "forge_api_url": forge,
        "preview_url": "http://localhost:1/pr-{pr}",
        "access": {"kind": "token"},
        "ci_accepted": true,
    });
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    serde_json::from_value(v).unwrap()
}

/// Sets `qid`'s link and both its tokens as the owner.
fn link(st: &Store, qid: &str, f: &RepoLinkFields) {
    let rev = st.set_queue_repo(CTX, qid, f, true).unwrap().rev;
    st.set_queue_repo_token(CTX, &kek(), qid, TokenKind::Write, WRITE, &rev)
        .unwrap();
    st.set_queue_repo_token(CTX, &kek(), qid, TokenKind::Read, READ, &rev)
        .unwrap();
}

/// The two pull request routes of a forge, counting calls.
#[derive(Clone, Default)]
struct Pulls {
    calls: Arc<AtomicUsize>,
}

async fn pulls(State(p): State<Pulls>, request: Request) -> Response {
    p.calls.fetch_add(1, Ordering::SeqCst);
    if request.method() == Method::GET {
        return axum::Json(json!([])).into_response();
    }
    let pr = json!({"number": 1, "html_url": "https://forge.example/pr/1"});
    (axum::http::StatusCode::CREATED, axum::Json(pr)).into_response()
}

async fn forge() -> (Pulls, String) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let p = Pulls::default();
    let app = Router::new().fallback(pulls).with_state(p.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (p, base)
}

/// A token link's whole world on a local daemon.
struct Linked {
    world: World,
    remote: Remote,
    pulls: Pulls,
    st: Arc<Store>,
    qid: String,
    _home: TempDir,
}

impl Linked {
    /// A link of `extra`'s on a remote open to anyone.
    async fn new(extra: Value) -> Linked {
        Self::on("", extra).await
    }

    /// A link of `extra`'s on a remote that takes only `token`.
    async fn on(token: &str, extra: Value) -> Linked {
        let world = World::new();
        let remote = serve(world.root.path(), token, None).await;
        let (pulls, forge) = forge().await;
        let (_home, st, qid) = store(BaseUrlPolicy::Local);
        link(&st, &qid, &fields(&remote.url(), &forge, &extra));
        Linked {
            world,
            remote,
            pulls,
            st,
            qid,
            _home,
        }
    }

    fn workspace(&self, egress: Egress) -> ContainerWorkspace {
        let source = StoreTargets::new(self.st.clone(), kek(), egress);
        bare(&self.world.runtime, vec![]).with_source(Arc::new(source))
    }

    /// [`Self::workspace`], remembering runs across a restart.
    fn keeping(&self, egress: Egress) -> ContainerWorkspace {
        let source = StoreTargets::new(self.st.clone(), kek(), egress);
        ContainerWorkspace::keeping(
            Arc::new(self.world.runtime.clone()),
            ContainerSpec::default(),
            vec![],
            false,
            self.world.root.path().join("ledger"),
            self.world.secrets.clone(),
        )
        .with_source(Arc::new(source))
    }

    fn queue(&self) -> Option<&str> {
        Some(&self.qid)
    }

    fn view(&self) -> clax_store::queue_repos::RepoLink {
        self.st.queue_repo(&self.qid).unwrap().unwrap()
    }

    async fn edit(&self, w: &ContainerWorkspace, r: &clax_store::bot_runs::Run, path: &str) {
        let command = format!("mkdir -p $(dirname {path}) && echo edited > {path}");
        let done = w.exec(r, self.queue(), &command, None, 30).await.unwrap();
        assert_eq!(done["exit_code"], 0, "{done}");
    }
}

fn bare(runtime: &Local, targets: Vec<Target>) -> ContainerWorkspace {
    ContainerWorkspace::new(
        Arc::new(runtime.clone()),
        ContainerSpec::default(),
        targets,
        false,
    )
}

#[tokio::test]
async fn a_token_link_drives_a_run_from_its_clone_to_a_pushed_commit() {
    let l = Linked::new(json!({"public": false})).await;
    assert_eq!(l.view().last_ok_at, None, "not tried yet");
    let w = l.workspace(Egress::Held);
    let r = run("r1");
    l.edit(&w, &r, "app.txt").await;
    assert_eq!(l.view().last_ok_at, None, "a clone says nothing of writing");

    let done = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(done["pr"], 1);
    assert!(l.view().last_ok_at.is_some(), "a real push worked");
    assert_eq!(
        file_at(&l.world.bare, "claxbot/thread1", "app.txt"),
        "edited"
    );
    assert_eq!(l.view().last_failure, None);
    let seen = l.remote.seen.lock().unwrap();
    assert!(seen.authorizations.contains(&basic(WRITE)), "the host's");
    assert!(
        seen.authorizations.contains(&basic(READ)),
        "the container's"
    );
    let execs = l.world.runtime.execs();
    let in_container = format!("{execs:?}");
    assert!(!in_container.contains(WRITE));
}

#[tokio::test]
async fn a_link_is_confidential_unless_it_says_public_and_carries_what_it_said() {
    let of = async |extra: Value| {
        let l = Linked::new(extra).await;
        l.workspace(Egress::Held).offer(&run("r1"), l.queue())
    };
    assert_eq!(of(json!({})).await, Offer::CHANGE.confidential());
    assert_eq!(of(json!({"public": true})).await, Offer::CHANGE);
    let offered = of(json!({"public": true, "reviewed_previews": true, "wake_on_reply": true}));
    assert_eq!(
        offered.await,
        Offer::CHANGE.reviewed_previews().wake_on_reply()
    );
}

#[tokio::test]
async fn a_confidential_link_runs_only_where_egress_is_held_or_said_to_be_open() {
    let closed = Linked::new(json!({})).await;
    let r = run("r1");
    assert_eq!(
        closed.workspace(Egress::Open).offer(&r, closed.queue()),
        Offer::default(),
        "the store would refuse it, so no run is told of it"
    );
    assert_eq!(
        closed.workspace(Egress::Held).offer(&r, closed.queue()),
        Offer::CHANGE.confidential()
    );
    let open = Linked::new(json!({"allow_open_egress": true})).await;
    assert_eq!(
        open.workspace(Egress::Open).offer(&r, open.queue()),
        Offer::CHANGE.confidential()
    );
    let public = Linked::new(json!({"public": true})).await;
    assert_eq!(
        public.workspace(Egress::Open).offer(&r, public.queue()),
        Offer::CHANGE
    );
}

#[tokio::test]
async fn a_run_that_has_begun_keeps_its_offer_until_it_ends_and_a_new_one_gets_nothing() {
    let l = Linked::new(json!({})).await;
    let w = l.workspace(Egress::Held);
    let r = run("r1");
    l.edit(&w, &r, "app.txt").await;
    l.st.clear_queue_repo(CTX, &l.qid).unwrap();
    assert_eq!(w.offer(&r, l.queue()), Offer::CHANGE.confidential());
    assert_eq!(w.offer(&run("r2"), l.queue()), Offer::default());
    w.finish(&r).await;
    assert_eq!(w.offer(&r, l.queue()), Offer::default(), "it ended");
}

#[tokio::test]
async fn a_restart_leaves_a_run_its_answers_and_its_shell_once_its_link_has_gone() {
    let l = Linked::new(json!({})).await;
    let w = l.keeping(Egress::Held);
    let r = run("r1");
    l.edit(&w, &r, "app.txt").await;
    let done = w.propose(&r, &proposal()).await.unwrap();
    l.st.clear_queue_repo(CTX, &l.qid).unwrap();

    let after = l.keeping(Egress::Held);
    assert_eq!(after.offer(&r, l.queue()), Offer::CHANGE.confidential());
    assert_eq!(after.propose(&r, &proposal()).await.unwrap(), done);
    let seen = after.exec(&r, l.queue(), "cat app.txt", None, 30).await;
    assert_eq!(seen.unwrap()["output"], "edited\n");
    let e = after.propose(&r, &another_proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED, "{}", e.message);
    assert!(e.message.contains("credentials"), "{}", e.message);
    assert!(e.message.contains("claxbot/thread1"), "{}", e.message);
}

/// A second bare repository beside the link's, `other.git`, holding the same
/// commits, and the link moved to it and made ready again by the owner.
fn rebind(l: &Linked) -> std::path::PathBuf {
    let root = l.world.root.path();
    let other = root.join("other.git");
    let from = l.world.bare.to_str().unwrap();
    crate::fixture::git(
        root,
        &["clone", "--quiet", "--bare", from, other.to_str().unwrap()],
    );
    let link = l.view();
    let url = format!("{}/other.git", l.remote.base);
    let moved = json!({"if_rev": link.rev, "repo": "o/other"});
    let forge = link.forge_api_url;
    let rev =
        l.st.set_queue_repo(CTX, &l.qid, &fields(&url, &forge, &moved), true);
    let again = json!({"if_rev": rev.unwrap().rev, "repo": "o/other"});
    let f = fields(&url, &forge, &again);
    let rev = l.st.set_queue_repo(CTX, &l.qid, &f, true).unwrap().rev;
    for (kind, token) in [(TokenKind::Write, WRITE), (TokenKind::Read, READ)] {
        l.st.set_queue_repo_token(CTX, &kek(), &l.qid, kind, token, &rev)
            .unwrap();
    }
    assert_eq!(l.view().status, LinkStatus::Ready);
    other
}

/// What `bare` holds under `claxbot/`.
fn claxbot_refs(bare: &std::path::Path) -> String {
    crate::fixture::git(bare, &["for-each-ref", "refs/heads/claxbot/"])
}

#[tokio::test]
async fn a_restart_does_not_carry_a_run_onto_the_repository_its_link_moved_to() {
    let l = Linked::new(json!({})).await;
    let w = l.keeping(Egress::Held);
    let r = run("r1");
    l.edit(&w, &r, "app.txt").await;
    let other = rebind(&l);

    let after = l.keeping(Egress::Held);
    let e = after.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED, "{}", e.message);
    assert!(e.message.contains("claxbot/thread1"), "{}", e.message);
    assert_eq!(
        claxbot_refs(&other),
        "",
        "nothing reached the new repository"
    );
    assert_eq!(l.pulls.calls.load(Ordering::SeqCst), 0);
    let seen = after.exec(&r, l.queue(), "cat app.txt", None, 30).await;
    assert_eq!(seen.unwrap()["output"], "edited\n", "it keeps its shell");
    assert_eq!(after.offer(&r, l.queue()), Offer::CHANGE.confidential());
}

#[tokio::test]
async fn a_command_with_no_answer_does_not_carry_a_run_onto_the_repository_its_link_moved_to() {
    let l = Linked::new(json!({})).await;
    let w = l.workspace(Egress::Held);
    let r = run("r1");
    l.edit(&w, &r, "app.txt").await;
    let other = rebind(&l);
    l.world.runtime.with(|b| b.stopped = true);
    let e = w.exec(&r, l.queue(), "true", None, 30).await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE, "{}", e.message);

    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED, "{}", e.message);
    assert_eq!(
        claxbot_refs(&other),
        "",
        "nothing reached the new repository"
    );
    assert_eq!(claxbot_refs(&l.world.bare), "", "and nothing was pushed");
}

#[tokio::test]
async fn a_link_that_stops_being_public_tightens_a_run_that_has_begun() {
    let l = Linked::new(json!({"public": true})).await;
    let w = l.workspace(Egress::Held);
    let r = run("r1");
    l.edit(&w, &r, "app.txt").await;
    assert_eq!(w.offer(&r, l.queue()), Offer::CHANGE);

    // A private link waits for its CI to be accepted, so no new run may use it.
    let link = l.view();
    let f = fields(
        &link.repo_url,
        &link.forge_api_url,
        &json!({"if_rev": link.rev, "public": false, "ci_accepted": false}),
    );
    l.st.set_queue_repo(CTX, &l.qid, &f, false).unwrap();
    assert_ne!(l.view().status, LinkStatus::Ready);
    assert_eq!(w.offer(&r, l.queue()), Offer::CHANGE.confidential());
}

#[tokio::test]
async fn a_link_made_public_loosens_no_run_that_has_begun() {
    let l = Linked::new(json!({})).await;
    let w = l.workspace(Egress::Held);
    let r = run("r1");
    l.edit(&w, &r, "app.txt").await;
    let link = l.view();
    let f = fields(
        &link.repo_url,
        &link.forge_api_url,
        &json!({"if_rev": link.rev, "public": true}),
    );
    l.st.set_queue_repo(CTX, &l.qid, &f, true).unwrap();
    assert_eq!(w.offer(&r, l.queue()), Offer::CHANGE.confidential());
}

#[tokio::test]
async fn a_push_over_someone_elses_commit_is_refused_and_blames_no_credential() {
    let l = Linked::new(json!({})).await;
    let w = l.workspace(Egress::Held);
    let r = run("r1");
    l.edit(&w, &r, "app.txt").await;
    l.world.someone_pushes("claxbot/thread1", "main");
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED, "{}", e.message);
    assert!(!e.retryable);
    assert_eq!(l.view().last_failure, None);
}

#[tokio::test]
async fn disconnecting_stops_new_runs() {
    let l = Linked::new(json!({})).await;
    let w = l.workspace(Egress::Held);
    assert_eq!(w.offer(&run("r1"), l.queue()), Offer::CHANGE.confidential());
    l.st.clear_queue_repo(CTX, &l.qid).unwrap();
    assert_eq!(w.offer(&run("r1"), l.queue()), Offer::default());
}

#[tokio::test]
async fn a_failed_publish_stays_on_show_through_a_change_of_read_token() {
    let l = Linked::new(json!({})).await;
    let rev = l.view().rev;
    l.st.note_repo_failure(&l.qid, None, PUBLISH_FAILED)
        .unwrap();
    let other = "github_pat_READ_9876543210-read";
    l.st.set_queue_repo_token(CTX, &kek(), &l.qid, TokenKind::Read, other, &rev)
        .unwrap();
    assert_eq!(l.view().last_failure.as_deref(), Some(PUBLISH_FAILED));
    let other = "github_pat_WRITE_9876543210-rite";
    l.st.set_queue_repo_token(CTX, &kek(), &l.qid, TokenKind::Write, other, &rev)
        .unwrap();
    assert_eq!(l.view().last_failure, None, "a new write token may publish");
}

#[tokio::test]
async fn a_link_waiting_for_the_owner_is_not_offered() {
    let l = Linked::new(json!({})).await;
    l.st.set_repo_token_approval(CTX, true).unwrap();
    let rev = l.view().rev;
    l.st.approve_queue_repo(CTX, &l.qid, &rev, false).unwrap();
    assert_eq!(
        l.workspace(Egress::Held).offer(&run("r1"), l.queue()),
        Offer::default()
    );
}

#[tokio::test]
async fn a_links_protected_paths_stand_on_top_of_the_defaults() {
    let l = Linked::new(json!({"protected_paths": ["infra/"]})).await;
    let w = l.workspace(Egress::Held);
    for (n, path) in ["infra/main.tf", ".github/ci.yml"].iter().enumerate() {
        let r = run(&format!("r{n}"));
        l.edit(&w, &r, path).await;
        let e = w.propose(&r, &proposal()).await.unwrap_err();
        assert_eq!(e.code, CODE_CHANGE_REFUSED, "{path}: {}", e.message);
        assert!(e.message.contains("protected"), "{path}: {}", e.message);
    }
    assert_eq!(l.view().last_failure, None, "a refusal is no credential's");
}

#[tokio::test]
async fn a_host_clone_that_fails_is_shown_and_tells_the_run_nothing_of_it() {
    let world = World::new();
    let (_pulls, forge) = forge().await;
    let (_home, st, qid) = store(BaseUrlPolicy::Local);
    link(
        &st,
        &qid,
        &fields("http://127.0.0.1:1/r.git", &forge, &json!({})),
    );
    let source = StoreTargets::new(st.clone(), kek(), Egress::Held);
    let w = bare(&world.runtime, vec![]).with_source(Arc::new(source));

    let e = w
        .exec(&run("r1"), Some(&qid), "true", None, 30)
        .await
        .unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(e.retryable, "nothing refused it");
    assert!(!e.message.contains("127.0.0.1"), "{}", e.message);
    let view = st.queue_repo(&qid).unwrap().unwrap();
    assert_eq!(view.last_ok_at, None);
    let failure = view.last_failure.unwrap();
    assert!(failure.contains("could not clone"), "{failure}");
    assert!(!failure.contains("127.0.0.1"), "{failure}");
}

#[tokio::test]
async fn credentials_that_do_not_open_are_shown() {
    let l = Linked::new(json!({})).await;
    let another = Kek::from_bytes([8; 32]);
    let source = StoreTargets::new(l.st.clone(), another, Egress::Held);
    let w = bare(&l.world.runtime, vec![]).with_source(Arc::new(source));
    let e = w
        .exec(&run("r1"), l.queue(), "true", None, 30)
        .await
        .unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    let failure = l.view().last_failure.unwrap();
    assert!(failure.contains("could not open"), "{failure}");
}

#[tokio::test]
async fn a_container_clone_the_remote_refuses_is_shown() {
    let l = Linked::on(WRITE, json!({})).await;
    let w = l.workspace(Egress::Held);
    let e = w
        .exec(&run("r1"), l.queue(), "true", None, 30)
        .await
        .unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(!e.retryable, "{}", e.message);
    assert!(e.message.contains("credentials"), "{}", e.message);
    let failure = l.view().last_failure.unwrap();
    assert!(failure.contains("container could not clone"), "{failure}");
}

#[tokio::test]
async fn the_operators_file_wins_over_a_link_on_the_same_queue() {
    let l = Linked::new(json!({})).await;
    let file = Target {
        queue: l.qid.clone(),
        ..l.world.target()
    };
    let source = StoreTargets::new(l.st.clone(), kek(), Egress::Held);
    let w = bare(&l.world.runtime, vec![file]).with_source(Arc::new(source));
    assert_eq!(
        w.offer(&run("r1"), l.queue()),
        Offer::CHANGE,
        "not confidential"
    );
    assert_eq!(w.operator_queues(), std::slice::from_ref(&l.qid));
    l.edit(&w, &run("r1"), "app.txt").await;
    assert_eq!(l.view().last_ok_at, None, "the link was not what ran");
    assert!(l.remote.seen.lock().unwrap().authorizations.is_empty());
}

struct Broken;

#[async_trait::async_trait]
impl TargetSource for Broken {
    fn describe(&self, _queue: &str) -> anyhow::Result<Option<Described>> {
        anyhow::bail!("the store is out of reach")
    }

    async fn open(&self, _queue: &str) -> anyhow::Result<Option<Target>> {
        anyhow::bail!("the store is out of reach")
    }

    fn succeeded(&self, _queue: &str, _stamp: &str, _used: RepoUse) {}

    fn failed(&self, _queue: &str, _stamp: Option<&str>, _what: &str) {}
}

#[test]
fn a_target_that_cannot_be_read_is_not_known_to_be_private() {
    let world = World::new();
    let w = ContainerWorkspace::new(
        Arc::new(world.runtime.clone()),
        ContainerSpec::default(),
        Vec::new(),
        false,
    )
    .with_source(Arc::new(Broken));
    assert!(w.offer(&run("r1"), Some("q")).confidential);
    assert!(!w.target_private(&run("r1"), Some("q")));
}

#[test]
fn a_public_run_whose_target_cannot_be_read_is_not_known_to_be_private() {
    let world = World::new();
    let dir = tempfile::tempdir().unwrap();
    let entry = clax_bot_workspace::ledger::Entry {
        offer: Some(clax_bot::workspace::Offer::CHANGE),
        ..Default::default()
    };
    let ledger = clax_bot_workspace::ledger::Ledger::at(dir.path().into());
    ledger.write("r1", &entry).unwrap();
    let w = ContainerWorkspace::keeping(
        Arc::new(world.runtime.clone()),
        ContainerSpec::default(),
        Vec::new(),
        false,
        dir.path().into(),
        Arc::default(),
    )
    .with_source(Arc::new(Broken));
    assert!(w.offer(&run("r1"), Some("q")).confidential);
    assert!(!w.target_private(&run("r1"), Some("q")));
}

#[tokio::test]
async fn a_lookup_that_fails_leaves_a_run_confidential_with_no_shell() {
    let runtime = World::new();
    let w = ContainerWorkspace::new(
        Arc::new(runtime.runtime.clone()),
        ContainerSpec::default(),
        vec![],
        true,
    )
    .with_source(Arc::new(Broken));
    assert_eq!(w.offer(&run("r1"), Some("q")), Offer::unsure());
}

#[tokio::test]
async fn a_daemon_with_no_source_has_no_link() {
    let l = Linked::new(json!({})).await;
    let w = bare(&l.world.runtime, vec![]);
    assert_eq!(w.offer(&run("r1"), l.queue()), Offer::default());
}

/// What a resolver says, changed by a test.
#[derive(Default)]
struct Switched {
    private: AtomicBool,
    asked: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Names for Switched {
    async fn lookup(&self, host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
        self.asked.lock().unwrap().push(host.to_string());
        let answer = if self.private.load(Ordering::SeqCst) {
            "10.0.0.8"
        } else {
            "127.0.0.1"
        };
        Ok(vec![answer.parse().unwrap()])
    }
}

/// A rule that takes this test's loopback for the public internet.
fn rule(names: &Arc<Switched>) -> PublicOnly {
    PublicOnly::with_names(names.clone()).calling_public(|ip| ip.is_loopback())
}

/// The port of `http://host:port`.
fn port_of(base: &str) -> &str {
    base.rsplit(':').next().unwrap()
}

#[tokio::test]
async fn every_git_command_goes_to_the_pinned_address_and_is_checked_anew() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let remote = serve(root.path(), WRITE, None).await;
    // Nothing resolves `.invalid` but the pin.
    let url = format!("http://forge.invalid:{}/repo.git", port_of(&remote.base));
    let names = Arc::new(Switched::default());
    let token = Token::new(WRITE);
    let dir = root.path().join("clone");
    let clone = Clone::reaching(&url, &dir, Some(&token), Some(rule(&names)))
        .await
        .unwrap();
    clone.start_branch("work", "main").await.unwrap();
    clone.fetch_branch("main", Some(&token)).await.unwrap();
    clone.push("work", Some(&token)).await.unwrap();
    assert!(names.asked.lock().unwrap().len() >= 3, "each is resolved");

    names.private.store(true, Ordering::SeqCst);
    let before = remote.seen.lock().unwrap().authorizations.len();
    let e = clone.push("work", Some(&token)).await.unwrap_err();
    assert!(e.to_string().contains("no public address"), "{e:#}");
    assert_eq!(remote.seen.lock().unwrap().authorizations.len(), before);
}

#[tokio::test]
async fn a_held_clone_with_no_token_still_follows_no_redirect() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let elsewhere = serve(root.path(), "", None).await;
    let moved = serve(root.path(), "", Some(elsewhere.base.clone())).await;
    let url = format!(
        "http://forge.invalid:{}/moved/repo.git",
        port_of(&moved.base)
    );
    let names = Arc::new(Switched::default());
    let dir = root.path().join("clone");
    let cloned = Clone::reaching(&url, &dir, None, Some(rule(&names))).await;
    assert!(cloned.is_err(), "a redirect was followed");
}

/// A link on a daemon held to public addresses.
async fn held() -> Linked {
    let world = World::new();
    let remote = serve(world.root.path(), "", None).await;
    let (pulls, forge) = forge().await;
    let (_home, st, qid) = store(BaseUrlPolicy::HostedLoopbackForTests);
    let f = fields(
        &format!("http://localhost:{}/repo.git", port_of(&remote.base)),
        &format!("http://localhost:{}", port_of(&forge)),
        &json!({"preview_url": "https://pr-{pr}.preview.example"}),
    );
    link(&st, &qid, &f);
    Linked {
        world,
        remote,
        pulls,
        st,
        qid,
        _home,
    }
}

fn held_workspace(l: &Linked, names: &Arc<Switched>) -> ContainerWorkspace {
    let source = StoreTargets::reaching(l.st.clone(), kek(), Egress::Held, rule(names));
    bare(&l.world.runtime, vec![]).with_source(Arc::new(source))
}

#[tokio::test]
async fn a_name_that_resolves_to_a_private_address_is_refused_before_git_starts() {
    let names = Arc::new(Switched::default());
    let l = held().await;
    names.private.store(true, Ordering::SeqCst);
    let w = held_workspace(&l, &names);
    let e = w
        .exec(&run("r1"), l.queue(), "true", None, 30)
        .await
        .unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert!(l.remote.seen.lock().unwrap().authorizations.is_empty());
    assert!(l.view().last_failure.is_some());
}

#[tokio::test]
async fn an_answer_that_changes_after_the_clone_stops_the_forges_requests() {
    let names = Arc::new(Switched::default());
    let l = held().await;
    let w = held_workspace(&l, &names);
    let r = run("r1");
    l.edit(&w, &r, "app.txt").await;
    assert!(!names.asked.lock().unwrap().is_empty());

    names.private.store(true, Ordering::SeqCst);
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_SHELL_UNAVAILABLE);
    assert_eq!(l.pulls.calls.load(Ordering::SeqCst), 0);
}

/// An App link on a fake forge that serves its repository too.
async fn app_link(extra: Value) -> (World, Fake, Arc<Store>, String, TempDir) {
    let world = World::new();
    let f = Fake::start(&world.bare).await;
    f.on_the_system_clock();
    let (home, st, qid) = store(BaseUrlPolicy::Local);
    let app = GithubAppFields {
        app_id: github::APP_ID,
        slug: "clax".into(),
        api_url: Some(f.base.clone()),
    };
    st.set_github_app(CTX, &kek(), &app, Some(github::KEY))
        .unwrap();
    let mut v = json!({
        "repo": "o/r",
        "preview_url": "http://localhost:1/pr-{pr}",
        "access": {"kind": "github_app"},
        "ci_accepted": true,
    });
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    let mut fields: RepoLinkFields = serde_json::from_value(v).unwrap();
    fields.access = RepoAccess::GithubApp {
        installation_id: github::INSTALLATION,
        account: "o".into(),
    };
    st.set_queue_repo(CTX, &qid, &fields, true).unwrap();
    (world, f, st, qid, home)
}

fn app_workspace(world: &World, st: &Arc<Store>) -> ContainerWorkspace {
    let source = StoreTargets::new(st.clone(), kek(), Egress::Held);
    let source = source.with_verify_delay(Duration::ZERO);
    bare(&world.runtime, vec![]).with_source(Arc::new(source))
}

#[tokio::test]
async fn an_app_link_commits_through_the_forge_and_the_container_gets_a_read_token_it_loses() {
    let (world, f, st, qid, _home) = app_link(json!({})).await;
    let w = app_workspace(&world, &st);
    let r = run("r1");
    let q = Some(qid.as_str());
    assert_eq!(w.offer(&r, q), Offer::CHANGE.confidential());
    let done = w
        .exec(&r, q, "echo edited > app.txt", None, 30)
        .await
        .unwrap();
    assert_eq!(done["exit_code"], 0, "{done}");
    let done = w.propose(&r, &proposal()).await.unwrap();
    assert_eq!(done["pr"], 1);
    assert_eq!(file_at(&world.bare, "claxbot/thread1", "app.txt"), "edited");
    assert_eq!(f.commits_made(), 1, "the forge made it");

    let (read, write) = f.tokens();
    assert_eq!(read.len(), 1, "one clone, one read token");
    assert_eq!(f.revoked(), read, "and it was ended");
    let held = format!("{:?}", world.runtime.execs());
    assert!(!write.iter().any(|t| held.contains(t)), "{held}");
    assert!(!held.contains("PRIVATE KEY"), "{held}");
    assert!(st.queue_repo(&qid).unwrap().unwrap().last_ok_at.is_some());
}

#[tokio::test]
async fn an_app_link_publishes_no_commit_the_forge_does_not_verify() {
    let (world, f, st, qid, _home) = app_link(json!({})).await;
    f.never_verifies();
    let w = app_workspace(&world, &st);
    let r = run("r1");
    let q = Some(qid.as_str());
    w.exec(&r, q, "echo edited > app.txt", None, 30)
        .await
        .unwrap();
    let e = w.propose(&r, &proposal()).await.unwrap_err();
    assert_eq!(e.code, CODE_CHANGE_REFUSED);
    assert!(e.message.contains("sign"), "{}", e.message);
}

#[tokio::test]
async fn a_public_app_link_gives_a_container_no_token() {
    let (world, f, st, qid, _home) = app_link(json!({"public": true})).await;
    f.opens_git_to_anyone();
    let w = app_workspace(&world, &st);
    let done = w
        .exec(&run("r1"), Some(&qid), "true", None, 30)
        .await
        .unwrap();
    assert_eq!(done["exit_code"], 0);
    assert!(f.tokens().0.is_empty());
}

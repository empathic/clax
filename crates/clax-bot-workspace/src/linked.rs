//! Targets a queue's admin connected in Clax, looked up when a run needs one.
//! `workspace.toml` names an operator's targets at boot; these live in the
//! store ([`clax_store::queue_repos`]) and change while the daemon runs, so
//! [`ContainerWorkspace`](crate::workspace::ContainerWorkspace) asks a
//! [`TargetSource`] each time:
//!
//! - [`TargetSource::describe`] answers from metadata, opens nothing and takes
//!   no time, for the workspace's synchronous `offer`.
//! - [`TargetSource::open`] opens the credentials, per run. A credential
//!   cleared later is not taken back from a run that already holds it, and the
//!   next run does not get it.
//!
//! An App link commits through the forge, whose commits are signed. A token
//! link pushes with `git`, whose commits are not: GitHub signs only for an
//! App, so it says [`UnsignedCommits::Allow`], and a repository that requires
//! signatures refuses the push.
//!
//! A link's addresses are a person's, so [`net`](crate::net) holds them to
//! public addresses unless its owner may reach their own machine
//! ([`ReadyTarget::local_forge`], [`ReadyTarget::local_preview`]).

use crate::forge::{GitHub, UnsignedCommits};
use crate::github_app::{GitHubApp, GitHubAppAccess};
use crate::net::{PublicOnly, Reach};
use crate::preview::Preview;
use crate::repo::{Policy, RepoAccess, StaticAccess, Token};
use crate::runtime::control_client;
use crate::workspace::Target;
use anyhow::{Context, Result};
use clax_store::Store;
use clax_store::queue_repos::{
    Egress, LinkStatus, ReadyTarget, RepoCredentials, RepoLink, RepoPreview, RepoUse,
};
use clax_store::secrets::Kek;
use std::sync::Arc;
use std::time::Duration;

/// What a target says of itself with nothing opened: the facts an offer and
/// a preview's wait stand on.
#[derive(Clone)]
pub struct Described {
    pub confidential: bool,
    pub reviewed_previews: bool,
    pub wake_on_reply: bool,
    /// That a reporter's word may start a run ([`Policy::follow_ups`]).
    pub follow_ups: bool,
    /// [`Policy::guidance`].
    pub guidance: Option<String>,
    /// [`Policy::preview_gate`].
    pub preview_gate: Option<String>,
    pub preview: Preview,
    /// What looks at its previews.
    pub preview_http: reqwest::Client,
}

/// Targets resolved per run.
#[async_trait::async_trait]
pub trait TargetSource: Send + Sync {
    /// `queue`'s target as of now, `None` when none can be used. An `Err`
    /// says nothing about the target: a workspace must then answer as for
    /// confidential material ([`clax_store::queue_repos::ReadyTarget`]).
    fn describe(&self, queue: &str) -> Result<Option<Described>>;

    /// Whether `queue`'s target says what a container holds of it is not for
    /// a reporter, usable or not: a run that has begun answers to this
    /// whatever else became of the target. No target says nothing.
    fn confidential(&self, queue: &str) -> Result<bool> {
        Ok(self.describe(queue)?.is_some_and(|d| d.confidential))
    }

    /// `queue`'s target with its credentials opened, for one run.
    async fn open(&self, queue: &str) -> Result<Option<Target>>;

    /// A real clone, commit or push with `queue`'s credentials worked:
    /// those of [`Target::stamp`], which says nothing of any others.
    fn succeeded(&self, queue: &str, stamp: &str, used: RepoUse);

    /// Using `queue`'s credentials failed: those of `stamp`, or as they stand
    /// for credentials that would not open. `what` is fixed words, shown to
    /// the people of the link.
    fn failed(&self, queue: &str, stamp: Option<&str>, what: &str);
}

/// The store's links, as a [`TargetSource`].
pub struct StoreTargets {
    store: Arc<Store>,
    kek: Kek,
    egress: Egress,
    public: PublicOnly,
    public_http: reqwest::Client,
    open_http: reqwest::Client,
    verify_delay: Option<Duration>,
}

impl StoreTargets {
    /// `store`'s links, opened with `kek`, under `egress`: what the running
    /// runtime does with its running spec, which a confidential link is held
    /// to.
    pub fn new(store: Arc<Store>, kek: Kek, egress: Egress) -> StoreTargets {
        StoreTargets::reaching(store, kek, egress, PublicOnly::default())
    }

    /// [`StoreTargets::new`], holding addresses to `public`.
    pub fn reaching(
        store: Arc<Store>,
        kek: Kek,
        egress: Egress,
        public: PublicOnly,
    ) -> StoreTargets {
        StoreTargets {
            public_http: public.client(),
            open_http: control_client(),
            verify_delay: None,
            public,
            store,
            kek,
            egress,
        }
    }

    /// These targets, their forge reading a commit again after `delay`.
    #[must_use]
    pub fn with_verify_delay(self, delay: Duration) -> StoreTargets {
        StoreTargets {
            verify_delay: Some(delay),
            ..self
        }
    }

    fn http(&self, local: bool) -> reqwest::Client {
        if local {
            self.open_http.clone()
        } else {
            self.public_http.clone()
        }
    }

    /// Whether `link` may run under this workspace's egress, as
    /// [`Store::open_queue_repo`] will decide it.
    fn may_run(&self, link: &RepoLink) -> bool {
        link.status == LinkStatus::Ready
            && (link.public || link.allow_open_egress || self.egress == Egress::Held)
    }

    /// What `ready`'s credentials give.
    fn access_of(&self, ready: &ReadyTarget, http: reqwest::Client) -> Result<Opened> {
        Ok(match &ready.credentials {
            RepoCredentials::GithubApp {
                app_id,
                installation_id,
                private_key,
            } => {
                let app = GitHubApp::new(&ready.forge_api_url, *app_id, private_key.expose())?;
                let app = app.with_http(http);
                let public = ready.public;
                let access = GitHubAppAccess::new(app, *installation_id, &ready.repo, public)?;
                Opened {
                    access: Arc::new(access),
                    committing: true,
                    unsigned: UnsignedCommits::Refuse,
                }
            }
            RepoCredentials::Token { read, write } => Opened {
                access: Arc::new(StaticAccess {
                    read: read.as_ref().map(|t| Token::new(t.expose())),
                    write: Some(Token::new(write.expose())),
                }),
                committing: false,
                unsigned: UnsignedCommits::Allow,
            },
        })
    }

    fn target_of(&self, ready: ReadyTarget) -> Result<Target> {
        let http = self.http(ready.local_forge);
        let opened = self.access_of(&ready, http.clone())?;
        let forge = GitHub::new(&ready.forge_api_url, &ready.repo, opened.access.clone());
        let forge = forge.with_http(http);
        let forge = match self.verify_delay {
            Some(delay) => forge.with_verify_delay(delay),
            None => forge,
        };
        let forge = if opened.committing {
            forge.committing()
        } else {
            forge
        };
        let mut policy = Policy::default();
        policy.protected_paths.extend(ready.protected_paths);
        Ok(Target {
            stamp: Some(ready.stamp),
            queue: ready.queue_id,
            container_repo_url: ready.repo_url.clone(),
            repo_url: ready.repo_url,
            base: ready.base,
            access: opened.access,
            reviewed_previews: ready.reviewed_previews,
            wake_on_reply: ready.wake_on_reply,
            unsigned: opened.unsigned,
            confidential: !ready.public,
            policy,
            forge: Arc::new(forge),
            preview: preview_of(&ready.preview),
            reach: Reach {
                git: (!ready.local_forge).then(|| self.public.clone()),
                preview: self.http(ready.local_preview),
            },
        })
    }
}

/// What a link's credentials give a [`Target`].
struct Opened {
    access: Arc<dyn RepoAccess>,
    committing: bool,
    unsigned: UnsignedCommits,
}

fn preview_of(p: &RepoPreview) -> Preview {
    Preview {
        url_template: p.url.clone(),
        ready_path: p.ready_path.clone(),
        ready_names_commit: p.names_commit,
        timeout: Duration::from_secs(p.timeout_secs),
        poll: Duration::from_secs(p.poll_secs),
    }
}

#[async_trait::async_trait]
impl TargetSource for StoreTargets {
    fn describe(&self, queue: &str) -> Result<Option<Described>> {
        let link = self.store.queue_repo(queue).context("reading the link")?;
        Ok(link.filter(|l| self.may_run(l)).map(|l| Described {
            confidential: !l.public,
            reviewed_previews: l.reviewed_previews,
            wake_on_reply: l.wake_on_reply,
            follow_ups: true,
            guidance: None,
            preview_gate: None,
            preview: preview_of(&l.preview),
            preview_http: self.http(l.local_preview),
        }))
    }

    fn confidential(&self, queue: &str) -> Result<bool> {
        let link = self.store.queue_repo(queue).context("reading the link")?;
        Ok(link.is_some_and(|l| !l.public))
    }

    async fn open(&self, queue: &str) -> Result<Option<Target>> {
        let ready = self.store.open_queue_repo(&self.kek, queue, self.egress);
        let ready = ready.context("opening the link")?;
        ready.map(|r| self.target_of(r)).transpose()
    }

    fn succeeded(&self, queue: &str, stamp: &str, used: RepoUse) {
        if let Err(e) = self.store.note_repo_ok(queue, stamp, used) {
            tracing::warn!(error = %e, queue, "noting a link's success failed");
        }
    }

    fn failed(&self, queue: &str, stamp: Option<&str>, what: &str) {
        if let Err(e) = self.store.note_repo_failure(queue, stamp, what) {
            tracing::warn!(error = %e, queue, "noting a link's failure failed");
        }
    }
}

//! The hooks a container runtime provides. Everything above this module
//! names only these traits: Daytona ([`crate::daytona`]) and Docker
//! ([`crate::docker`]) are two implementations, chosen by [`RuntimeConfig`],
//! and another is one more `impl` and one more arm of [`RuntimeConfig::open`].
//! `tests/conformance/contract.rs` is what each must pass.
//!
//! A [`Container`] is disposable and holds nothing the run would mind
//! losing to whatever ran in it. All that comes out of one is chosen by
//! whatever ran in it, so a runtime bounds what it reads and how long it
//! waits. What must be trusted about a container, such as the commit it
//! started from, is kept outside it: in its labels, which nothing inside can
//! change, and in the host's [`crate::ledger`].

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// The most of one command's output a host keeps, which also bounds a
/// patch. A container can write without end, and the host must not hold all
/// of it. Output this long has been cut.
pub const MAX_EXEC_OUTPUT: usize = 4 * 1024 * 1024;

/// The exit code of a command stopped for overrunning its timeout, as
/// `timeout(1)` gives it.
pub const EXIT_TIMED_OUT: i32 = 124;

/// How long a call to a control plane may take.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// [`control_client`], not yet built.
pub fn control_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(CONTROL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
}

/// A client for the APIs this crate calls: bounded in time, and following
/// no redirect, which would carry a credential or a probe somewhere unasked.
pub fn control_client() -> reqwest::Client {
    control_builder()
        .build()
        .expect("a client with no custom TLS builds")
}

/// `r`'s body, reading no further than `limit`: `None` when there was more.
pub async fn bounded(mut r: reqwest::Response, limit: usize) -> Result<Option<Vec<u8>>> {
    let mut body = Vec::new();
    while let Some(chunk) = r.chunk().await.context("reading an answer")? {
        if body.len() + chunk.len() > limit {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

/// A container that does not exist, or never will again. Any other error
/// from a runtime says nothing about the container, only that asking failed.
#[derive(Debug)]
pub struct Gone(pub String);

impl fmt::Display for Gone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Gone {}

/// Whether `e` says the container is [`Gone`].
pub fn is_gone(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Gone>().is_some()
}

/// One command for a container's shell.
#[derive(Clone)]
pub struct Exec {
    /// Shell text: pipes, `&&` and quoting mean what they mean to `sh`. Its
    /// stdin is empty.
    pub command: String,
    pub cwd: Option<String>,
    /// Set for this command alone. A value may be a token: a runtime keeps
    /// it out of `argv` and out of every log.
    pub envs: BTreeMap<String, String>,
    pub timeout: Duration,
}

impl Exec {
    pub fn new(command: impl Into<String>, timeout: Duration) -> Exec {
        Exec {
            command: command.into(),
            cwd: None,
            envs: BTreeMap::new(),
            timeout,
        }
    }

    pub fn in_dir(mut self, cwd: impl Into<String>) -> Exec {
        self.cwd = Some(cwd.into());
        self
    }
}

impl fmt::Debug for Exec {
    /// Names `envs` and shows no value of them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Exec")
            .field("command", &self.command)
            .field("cwd", &self.cwd)
            .field("envs", &self.envs.keys().collect::<Vec<_>>())
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// What a command did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_code: i32,
    /// Its output, stdout and stderr as the runtime merged them, at most
    /// [`MAX_EXEC_OUTPUT`].
    pub output: String,
}

/// A container [`ContainerRuntime::find`] answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub id: String,
    /// As they were given at creation.
    pub labels: BTreeMap<String, String>,
}

impl Found {
    /// Whether it carries all of `labels`.
    pub fn labels_hold(&self, labels: &BTreeMap<String, String>) -> bool {
        labels.iter().all(|(k, v)| self.labels.get(k) == Some(v))
    }
}

/// What a new container is made from.
#[derive(Clone, Debug, Default)]
pub struct ContainerSpec {
    /// The image or snapshot; `None` is the runtime's default.
    pub image: Option<String>,
    /// A name unique to this container, so that a create whose answer was
    /// lost can be checked; `None` leaves it to the runtime.
    pub name: Option<String>,
    pub labels: BTreeMap<String, String>,
    /// Idle minutes before the runtime stops it, so a crashed daemon does
    /// not leave one running. A command that runs for longer than this may
    /// be stopped with it.
    pub auto_stop_minutes: Option<u32>,
    /// The only hosts it may reach. Empty leaves the runtime's default,
    /// which may be open egress. A runtime that cannot hold a container to
    /// this list refuses to make one ([`ContainerRuntime::limits_domains`]).
    pub domain_allow_list: Vec<String>,
}

/// A running container.
#[async_trait::async_trait]
pub trait Container: Send + Sync {
    fn id(&self) -> &str;

    /// Runs `exec`. A command that fails is an [`ExecOutput`] with its exit
    /// code. One that overruns `exec.timeout` answers a failing code,
    /// [`EXIT_TIMED_OUT`] where the runtime can tell, a little after the
    /// timeout at most: that bound on the host's wait holds whatever the
    /// container does. In an honest container the command is stopped too,
    /// even if it ignores SIGTERM, but stopping it rests on tools the
    /// container can replace. An `Err` means it cannot be said that the
    /// command ran, as for a container that stopped.
    async fn exec(&self, exec: &Exec) -> Result<ExecOutput>;
}

/// Makes containers.
#[async_trait::async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// Whether it holds a container to [`ContainerSpec::domain_allow_list`].
    fn limits_domains(&self) -> bool;

    /// Whether anyone but this daemon's operator runs what holds a container,
    /// as a person connecting private source should be told. A runtime that
    /// cannot say answers yes.
    fn third_party(&self) -> bool {
        true
    }

    /// Makes a container and answers once it takes commands. Refuses a
    /// non-empty allow-list unless [`Self::limits_domains`], and deletes one
    /// that does not enforce it: a container asked to be limited never runs
    /// open.
    async fn create(&self, spec: &ContainerSpec) -> Result<Box<dyn Container>>;

    /// Container `id`, once it takes commands: one its runtime stopped is
    /// started again, with its filesystem as it was left. It must hold to
    /// `spec`'s allow-list as [`Self::create`] does, and is deleted if not.
    /// [`Gone`] when there is no such container to have.
    async fn attach(&self, id: &str, spec: &ContainerSpec) -> Result<Box<dyn Container>>;

    /// Every container of this runtime carrying all of `labels`, running or
    /// stopped. Finding none is an empty answer; an `Err` says nothing about
    /// whether there are any.
    async fn find(&self, labels: &BTreeMap<String, String>) -> Result<Vec<Found>>;

    /// Ends container `id`, without starting it first. Ending one already
    /// gone is fine.
    async fn destroy(&self, id: &str) -> Result<()>;
}

/// Which runtime a deployment uses: the only place one is named.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeConfig {
    /// `DAYTONA_API_KEY`, `DAYTONA_API_URL`, `DAYTONA_ORGANIZATION_ID`.
    Daytona,
    /// The `docker` on PATH.
    Docker {
        /// Extra `docker run` arguments, such as `--add-host`. They come
        /// after the defaults, so a `--memory` here replaces its default.
        #[serde(default)]
        run_args: Vec<String>,
    },
}

impl RuntimeConfig {
    pub fn open(&self) -> Result<Arc<dyn ContainerRuntime>> {
        Ok(match self {
            RuntimeConfig::Daytona => Arc::new(crate::daytona::Daytona::from_env()?),
            RuntimeConfig::Docker { run_args } => Arc::new(crate::docker::Docker {
                run_args: run_args.clone(),
            }),
        })
    }
}

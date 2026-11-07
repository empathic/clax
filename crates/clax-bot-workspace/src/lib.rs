//! The container machinery ClaxBot drives (spec
//! `docs/superpowers/specs/2026-11-06-claxbot-workspace-design.md`). ClaxBot's run
//! loop is the agent; this crate is what its `clax.shell.exec` and
//! `clax.change.propose` reach.
//!
//! - [`runtime`]: the hooks a container runtime provides; [`daytona`] and
//!   [`docker`] implement them; [`workspace`] is `clax_bot::Workspace` over them;
//! - [`linked`]: targets a queue's admin connected in Clax, resolved per run;
//!   [`net`]: where a link's requests may go;
//! - [`repo`]: the host's own clone, the only one that pushes, and tokens;
//!   [`github_app`]: short-lived tokens from a GitHub App;
//!   [`ledger`]: what the host remembers of a run across a restart;
//!   [`forge`]: pull requests, and commits a forge makes and signs;
//!   [`preview`]: a pull request's preview.

pub mod config;
pub mod daytona;
pub mod docker;
pub mod forge;
pub mod github_app;
pub mod ledger;
pub mod linked;
pub mod net;
pub mod preview;
pub mod repo;
pub mod runtime;
pub mod workspace;

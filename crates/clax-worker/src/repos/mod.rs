//! Artifact content for the Worker: versions in git, one repository per
//! artifact, through clax-content-git's `GitContent`, over one of two
//! hosts chosen by the `REPO_STORE` var:
//!
//! - `memory`: the stand-in, for `wrangler dev --local` without a
//!   Cloudflare account. Every repository's objects and refs are in one
//!   Durable Object, `RepoDO` (binding `REPO`, the instance named
//!   `repos`), in its own SQLite storage through `clax-do-vfs`
//!   ([`db`]). Callers reach it through its internal API ([`api`]); the
//!   Worker forwards `/git/<repo>.git/…` to it, where it serves clone,
//!   fetch and push over smart HTTP with repository tokens
//!   ([`git_http`]).
//! - `artifacts`: Cloudflare Artifacts through the `ARTIFACTS` binding,
//!   written over receive-pack to each repository's remote
//!   ([`artifacts`]). Artifacts serves its own git remotes.
//!
//! Either way assets are objects in the `ASSETS` R2 bucket ([`assets`]),
//! and `RepoDO` mints, revokes and keeps the ledger of repository tokens
//! ([`tokens`]), with the must-revoke list its alarm retries.
//!
//! The Worker's entry points are in `worker` (wasm32 only):
//! `content_store(env)`, the `ContentStore` an instance Durable Object
//! holds; `forward_git`, the `/git/` route; `mint_git_token` and
//! `revoke_git_token`; and `git_remotes(env)`, the server's
//! `clax_server::git_remotes::GitRemotes` ([`remotes`]), behind its git
//! token routes and the MCP `git_remote` tool.

pub mod api;
pub mod artifacts;
pub mod assets;
pub mod db;
pub mod git_http;
pub mod local;
pub mod remotes;
pub mod tokens;

#[cfg(target_arch = "wasm32")]
pub mod js;
#[cfg(target_arch = "wasm32")]
pub mod worker;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

/// Which host keeps the repositories: the `REPO_STORE` var.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoStore {
    /// The `RepoDO` stand-in.
    Memory,
    /// Cloudflare Artifacts.
    Artifacts,
}

impl RepoStore {
    /// `memory` or `artifacts`; anything else is `None`.
    pub fn parse(s: &str) -> Option<RepoStore> {
        match s {
            "memory" => Some(RepoStore::Memory),
            "artifacts" => Some(RepoStore::Artifacts),
            _ => None,
        }
    }
}

/// Milliseconds since the epoch, by the wall clock.
pub fn now_ms() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
    }
}

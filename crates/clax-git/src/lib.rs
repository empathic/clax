//! Git for Clax artifact versions, in pure Rust with no I/O of its own: it
//! compiles natively and for `wasm32-unknown-unknown` (a Cloudflare
//! Durable Object), and the caller supplies storage and HTTP.
//!
//! - [`object`], [`id`]: blob, tree and commit encoding and SHA-1 IDs,
//!   byte for byte git's; commit message trailers.
//! - [`tree`]: a new tree from a parent tree plus path changes.
//! - [`pack`], [`pktline`]: packfiles (writing whole objects; reading
//!   deltified and thin packs) and pkt-line framing.
//! - [`client`]: a receive-pack client, for remotes written only over git
//!   smart HTTP (Cloudflare Artifacts); requests and responses are data
//!   ([`http`]) moved by the caller's [`http::Transport`].
//! - [`server`]: upload-pack and receive-pack over a caller's
//!   [`repo::Repo`], so a local stand-in serves `git clone`, `fetch` and
//!   `push` over smart HTTP.
//! - [`repo`], [`memory`]: the repository interface (commits read as
//!   [`CommitMeta`], futures `Send` natively) and an in-memory host.
//! - [`versions`]: artifact versions as commits on `main` tagged `v<n>`:
//!   publish, read, list, fork.
//! - [`events`]: Cloudflare Artifacts push events.

pub mod client;
pub mod error;
pub mod events;
pub mod http;
pub mod id;
pub mod memory;
pub mod object;
pub mod pack;
pub mod pktline;
pub mod repo;
pub mod server;
pub mod tree;
pub mod versions;

pub use error::{GitError, Result};
pub use id::ObjectId;
pub use object::{Commit, Mode, Object, ObjectKind, Person, Signature, TreeEntry};
pub use repo::{
    CommitMeta, MAIN, MaybeSend, MaybeSendSync, RefStatus, RefUpdate, Repo, RepoHost,
    reachable_from,
};
pub use versions::{FileEntry, GitVersions, Publish, Published, Rev, VersionMeta};

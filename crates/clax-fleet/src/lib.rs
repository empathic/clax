//! Clax across processes: the server a Kubernetes deployment runs
//! (`deploy/k8s`, `docs/kubernetes.md`), serving the hosted Clax instances
//! its catalog lists ([`catalog`]), backed by SQLite or by PostgreSQL, and
//! scaled out by adding processes.
//!
//! # The model
//!
//! A Clax instance is one writer: its store's write connection, its
//! in-memory realtime state (streams, presence, seats) and its background
//! tasks belong to one process, as a Durable Object's do in the hosted
//! Worker (`crates/clax-worker`). The fleet keeps that: each instance runs
//! in exactly one process at a time, and the fleet scales by spreading
//! instances across processes. Any process takes any request: one for an
//! instance it runs is answered in process, any other is forwarded to the
//! process that runs it ([`proxy`]), WebSockets and event streams
//! included. What decides who runs what is the backend ([`fleet`]):
//!
//! - **SQLite** ([`fleet::Placement::Shards`], [`fleet::Storage::Disk`]): a
//!   StatefulSet. Each process has a volume of its own, and each instance
//!   names the shard (the process ordinal) whose volume keeps its home, a
//!   home like `clax serve`'s with the store's WAL, reader pool and
//!   checkpoints. Scaling out adds shards for new instances; an instance
//!   moves only when its home is moved and its shard changed with it.
//! - **PostgreSQL** ([`fleet::Placement::Leases`],
//!   [`fleet::Storage::Postgres`]): a Deployment of interchangeable
//!   processes, scaled by hand or by a HorizontalPodAutoscaler. An instance
//!   runs where its lease is held ([`coord`]); with no live lease it goes to
//!   the member rendezvous hashing prefers, so instances spread evenly and a
//!   process that joins or leaves moves only its share. Its database is
//!   SQLite on `clax-pagevfs` with the pages in PostgreSQL ([`pages`]), its
//!   artifact content ([`content`]) and its artifacts' app data
//!   ([`app_data`]) are in PostgreSQL too, and every write is fenced by the
//!   lease ([`pg::Fence`]), so a process that lost an instance can never
//!   write to it again. Nothing on a process's disk needs to survive it.
//!
//! An instance on one process serves as many requests as one process can;
//! the fleet scales the number of instances, not one instance's writes.

pub mod app_data;
pub mod catalog;
pub mod content;
pub mod coord;
pub mod fleet;
pub mod pages;
pub mod pg;
pub mod proxy;
pub mod tls;

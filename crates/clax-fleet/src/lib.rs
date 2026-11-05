//! Clax across processes: the server a Kubernetes deployment runs
//! (`deploy/k8s`, `docs/kubernetes.md`), serving the hosted Clax instances
//! its catalog lists ([`catalog`]), each on the volume of its shard, and
//! scaled out by adding shards.
//!
//! # The model
//!
//! A Clax instance is one writer: its store's write connection, its
//! in-memory realtime state (streams, presence, seats) and its background
//! tasks belong to one process, as a Durable Object's do in the hosted
//! Worker (`crates/clax-worker`). The fleet keeps that: each instance runs
//! in exactly one process, and the fleet scales by spreading instances
//! across processes. Any process takes any request: one for an instance it
//! runs is answered in process, any other is forwarded to the process that
//! runs it ([`proxy`]), WebSockets and event streams included.
//!
//! The processes are a StatefulSet's pods ([`fleet::Shard`]). Each has a
//! volume of its own, and each instance names the shard (the pod's
//! ordinal) whose volume keeps its home, a home like `clax serve`'s with
//! the store's WAL, reader pool and checkpoints. Scaling out adds shards
//! for new instances; an instance moves only when its home is moved and
//! its shard changed with it.
//!
//! An instance on one process serves as many requests as one process can;
//! the fleet scales the number of instances, not one instance's writes.

pub mod catalog;
pub mod fleet;
pub mod proxy;

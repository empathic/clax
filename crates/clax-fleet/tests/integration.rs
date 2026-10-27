//! The integration tests of `clax-fleet` in one test binary, a module per
//! file in `tests/`: fleets of several processes, each a [`Fleet`] served
//! on a port of its own in this one, on the disk storage (PostgreSQL's
//! tests are `tests/postgres.rs`).
//!
//! [`Fleet`]: clax_fleet::fleet::Fleet

mod common;
mod leases;
mod proxy;
mod shards;

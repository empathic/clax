//! The integration tests of `clax-fleet` in one test binary, a module per
//! file in `tests/`: fleets of several processes, each a [`Fleet`] served
//! on a port of its own in this one.
//!
//! [`Fleet`]: clax_fleet::fleet::Fleet

mod common;
mod proxy;
mod shards;

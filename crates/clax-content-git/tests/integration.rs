//! clax-content-git integration tests: one binary, a module per file.
#![warn(clippy::disallowed_methods)]

mod common;
mod contract;
mod over_http;
mod pushes;
mod races;
mod steps;
mod store_fork;
mod with_git;

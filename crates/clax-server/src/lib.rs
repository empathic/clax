//! The Clax daemon: the route handlers (`routes`), the router that mounts
//! them (`router`), the shell and its first-load data (`shell`), and the
//! process that serves a home (`serve`). What the routes share is
//! `clax-http`'s; the daemon's discovery files are `clax_core::daemon`.

pub mod router;
pub mod routes;
pub mod serve;
pub mod shell;

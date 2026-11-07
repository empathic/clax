//! The service over its real router and a real
//! [`clax_bot_workspace::workspace::ContainerWorkspace`], driven by
//! [`clax_bot_remote::RemoteWorkspace`] through a transport in this process,
//! on a fake runtime and a fake forge. Nothing here reaches a network, and a
//! test that waits does so on paused time.

#[path = "../../clax-bot/tests/common/mod.rs"]
mod common;
mod fixture;
mod jobs;
mod previews;
mod run_loop;
mod wire;

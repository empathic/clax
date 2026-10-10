// Every test binary declares `mod common`; not each uses all of it.
#[allow(dead_code)]
pub mod clients;
#[allow(dead_code)]
pub mod people;

pub use clax_server::testing::TestServer;

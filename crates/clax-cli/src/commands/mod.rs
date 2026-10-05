pub mod asset;
pub mod bin;
pub mod comments;
pub mod db;
pub mod delete;
pub mod doctor;
pub mod doctor_agent;
pub mod extension;
pub mod feedback;
pub mod haiku;
pub mod hook;
pub mod init;
pub mod list;
pub mod mcp;
pub mod native_host;
pub mod open;
pub mod pin;
pub mod publish;
pub mod read;
pub mod serve;
pub mod status;
pub mod stop;
pub mod tools;
pub mod versions;

use crate::client::Client;

pub fn daemon_json(c: &Client) -> serde_json::Value {
    serde_json::json!({"running": true, "port": c.info.port, "pid": c.info.pid, "url": c.browser_url("/"), "version": c.info.version, "bind": c.info.bind})
}

pub fn print(
    cli: &crate::Cli,
    json: serde_json::Value,
    text: impl FnOnce(&serde_json::Value) -> String,
) {
    if cli.json {
        println!("{json}");
    } else {
        println!("{}", text(&json));
    }
}

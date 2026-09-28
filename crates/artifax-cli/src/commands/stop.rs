use crate::client::Client;
use artifax_core::Home;

pub fn run(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    match Client::discover(home) {
        Some(c) => {
            c.shutdown()?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < deadline
                && artifax_server::daemon::pid_alive(c.info.pid)
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            super::print(
                cli,
                serde_json::json!({"stopped": true, "pid": c.info.pid}),
                |_| "artifax daemon stopped".into(),
            );
        }
        None => super::print(cli, serde_json::json!({"stopped": false}), |_| {
            "no artifax daemon is running".into()
        }),
    }
    Ok(())
}

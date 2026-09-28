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
            let stopped = !artifax_server::daemon::pid_alive(c.info.pid);
            if !stopped {
                eprintln!(
                    "warning: daemon (pid {}) is still draining connections",
                    c.info.pid
                );
            }
            super::print(
                cli,
                serde_json::json!({"stopped": stopped, "pid": c.info.pid}),
                |j| {
                    if stopped {
                        "artifax daemon stopped".into()
                    } else {
                        format!("artifax daemon (pid {}) is still shutting down", j["pid"])
                    }
                },
            );
        }
        None => super::print(cli, serde_json::json!({"stopped": false}), |_| {
            "no artifax daemon is running".into()
        }),
    }
    Ok(())
}

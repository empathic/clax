//! `clax passkeys`: the offline recovery of browser confirmations'
//! passkeys (docs/contract.md, "Browser confirmations", "Passkeys").
//!
//! `clax passkeys reset` removes every registered passkey and turns the
//! requirement off, straight in the home's store, so an owner whose
//! passkey cannot be used (one an agent registered, a lost authenticator,
//! a daemon now served at a LAN address) confirms by browser again. It
//! runs only while the home's daemon is stopped, holding the start lock so
//! none starts meanwhile, and prints what it removed.

use anyhow::bail;
use clax_core::Home;
use clax_core::daemon::{DaemonLock, pid_alive, read_daemon_info};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Remove every passkey and stop requiring one to confirm, with the
    /// daemon for this home stopped (`clax stop` first).
    Reset,
}

pub fn run(cli: &crate::Cli, home: &Home, c: &Cmd) -> anyhow::Result<()> {
    match c {
        Cmd::Reset => reset(cli, home),
    }
}

fn reset(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    // Held until this returns: no daemon starts on the home meanwhile.
    let Some(_lock) = DaemonLock::try_acquire(home)? else {
        bail!("a daemon for this home is starting: stop it (`clax stop`), then run this again");
    };
    if let Some(info) = read_daemon_info(home).filter(|i| pid_alive(i.pid)) {
        bail!(
            "the daemon for this home is running (pid {}): stop it (`clax stop`), then run this again",
            info.pid
        );
    }
    let st = clax_store::Store::open(home)?;
    let ctx = clax_core::audit::AuditCtx {
        actor: clax_core::audit::Actor::Owner {
            public_id: st.owner_viewer(false)?.public_id,
        },
        via: clax_core::audit::Via::Cli,
        git: clax_core::gitctx::GitField::Absent,
        call: None,
    };
    let removed = st.reset_passkeys(&ctx)?;
    super::print(
        cli,
        serde_json::json!({"removed": removed, "require_passkey": false}),
        |j| {
            let list = j["removed"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                return "no passkeys were registered; confirming does not require one".into();
            }
            let mut out = format!(
                "removed {} passkey{}:",
                list.len(),
                if list.len() == 1 { "" } else { "s" }
            );
            for p in &list {
                let by = &p["added_by"];
                let agents = by["agents_live"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|a| !a.is_empty())
                    .unwrap_or_else(|| "none".into());
                out.push_str(&format!(
                    "\n  {} added {} from {}; agents running then: {}",
                    crate::term::clean_line(p["credential_id"].as_str().unwrap_or("")),
                    crate::term::clean_line(p["created_at"].as_str().unwrap_or("")),
                    crate::term::clean_line(by["via"].as_str().unwrap_or("an unknown browser")),
                    crate::term::clean_line(&agents),
                ));
            }
            out.push_str("\nconfirming no longer requires a passkey");
            out
        },
    );
    Ok(())
}

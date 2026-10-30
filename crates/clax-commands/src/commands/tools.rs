//! Commands that print an MCP tool's result: they run the tool against the
//! daemon, so the CLI and the tools share one implementation and one JSON shape.

use crate::client::Client;
use clax_core::Home;
use clax_core::remote::SessionCredential;
use clax_mcp::client::ClientError;
use clax_mcp::{ClaxTools, DaemonClient};
use rmcp::model::CallToolResult;
use std::future::Future;

/// The tools' client for the daemon `c` reaches: an agent channel exactly
/// when the CLI is one ([`Client::agent`]), naming the CLI as its channel
/// (audit spec §6.9), and signing every request for a remote daemon. The
/// one `DaemonClient` the CLI makes (`crates/clax-commands/clippy.toml`).
fn daemon_client(c: &Client) -> DaemonClient {
    daemon_client_in(c, None)
}

/// [`daemon_client`] acting in `session` when given: every request names
/// it (`X-Clax-Session`) and, when the credential has a secret (a remote
/// daemon's grant session), proves it.
#[allow(clippy::disallowed_methods)]
fn daemon_client_in(c: &Client, session: Option<&InSession>) -> DaemonClient {
    let d = DaemonClient::with_channel(
        c.base.clone(),
        c.token.clone(),
        session.map(|s| s.id.clone()),
        c.agent,
    );
    let d = match c.remote() {
        Some(r) => d.with_remote(r.clone()),
        None => d,
    };
    match session.and_then(|s| s.secret.as_ref()) {
        Some(secret) => d.with_session_secret(secret.clone()),
        None => d,
    }
    .with_via("cli")
}

/// The agent session a command acts in: its ID and, on a remote daemon,
/// the secret that proves it (`None` on the local daemon, whose sessions
/// act with its token).
#[derive(Clone, Debug)]
pub struct InSession {
    pub id: String,
    pub secret: Option<clax_core::secret::Secret>,
}

impl InSession {
    /// Session `id`, proved by `cred` when there is one.
    pub fn new(id: String, cred: Option<&SessionCredential>) -> InSession {
        InSession {
            id,
            secret: cred.map(|c| c.secret.clone()),
        }
    }
}

/// Runs `f` with the tools' client for the daemon `c` reaches, acting in
/// `session` when given, and returns its answer as the client gave it:
/// for commands that make the client's calls themselves rather than run a
/// tool, whose result in a session carries the session's waiting feedback.
///
/// # Errors
/// When the runtime cannot be built.
pub fn daemon<T, F, Fut>(
    c: &Client,
    session: Option<&InSession>,
    f: F,
) -> anyhow::Result<clax_mcp::client::Result<T>>
where
    F: FnOnce(DaemonClient) -> Fut,
    Fut: Future<Output = clax_mcp::client::Result<T>>,
{
    let d = daemon_client_in(c, session);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    Ok(rt.block_on(f(d)))
}

/// The error a daemon call's failure `e` becomes: a refusal as `<code>:
/// <message>`, cleaned as the [`Client`]'s errors are; any other failure
/// as the client words it.
pub fn client_error(e: ClientError) -> anyhow::Error {
    match e {
        ClientError::Api { error, .. } => tool_error(&serde_json::json!({ "error": error })),
        e => anyhow::anyhow!("{}", crate::term::clean_line(&e.to_string())),
    }
}

/// Runs `f` with the tool set for the daemon (started if needed) and returns
/// the tool's result object. An error result becomes an error naming its
/// `code` and `message`.
pub fn call<F, Fut>(cli: &crate::Cli, home: &Home, f: F) -> anyhow::Result<serde_json::Value>
where
    F: FnOnce(ClaxTools) -> Fut,
    Fut: Future<Output = Result<CallToolResult, rmcp::ErrorData>>,
{
    let c = Client::connect(home, cli.port_for(home)?)?;
    call_on(&c, home, f)
}

/// [`call`] for the daemon `c` already reaches.
pub fn call_on<F, Fut>(c: &Client, home: &Home, f: F) -> anyhow::Result<serde_json::Value>
where
    F: FnOnce(ClaxTools) -> Fut,
    Fut: Future<Output = Result<CallToolResult, rmcp::ErrorData>>,
{
    let tools = ClaxTools::new(daemon_client(c), c.browser_url(""), None, home.log_path());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = rt
        .block_on(f(tools))
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .ok_or_else(|| anyhow::anyhow!("the tool returned no text"))?;
    let value: serde_json::Value = serde_json::from_str(&text)?;
    if result.is_error == Some(true) {
        return Err(tool_error(&value));
    }
    Ok(value)
}

/// The error an error result `value` becomes: `<code>: <message>`. Both
/// come from the daemon, so they are cleaned as the [`Client`]'s errors
/// are: neither can move the cursor or reorder the line.
fn tool_error(value: &serde_json::Value) -> anyhow::Error {
    let e = &value["error"];
    anyhow::anyhow!(
        "{}: {}",
        crate::term::clean_line(e["code"].as_str().unwrap_or("error")),
        crate::term::clean_line(e["message"].as_str().unwrap_or("the tool failed"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_error_is_cleaned_of_control_characters() {
        let e = tool_error(&serde_json::json!({
            "error": {"code": "bad\x1b[2Jcode", "message": "no\u{202e}pe\nnext"}
        }))
        .to_string();
        assert!(
            !e.chars().any(|c| c.is_control() || c == '\u{202e}'),
            "{e:?}"
        );
        assert!(e.contains("code") && e.contains("pe"), "{e:?}");
    }

    /// The commands that run a tool (`read`, `db`, `asset`) mark their
    /// requests as the CLI does: an agent channel under a harness, the
    /// owner by hand.
    #[test]
    fn the_tool_commands_follow_the_clis_channel() {
        let mut c = crate::client::tests::client_for_test();
        c.agent = true;
        assert!(daemon_client(&c).agent_channel());
        c.agent = false;
        assert!(!daemon_client(&c).agent_channel());
    }

    /// Every command reaches the daemon through [`Client`] or
    /// [`daemon_client`]: no other HTTP client, so none skips the marker.
    #[test]
    fn every_command_reaches_the_daemon_through_the_marked_clients() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![];
        for dir in [src.clone(), src.join("commands")] {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        assert!(files.len() > 20, "{}", files.len());
        for p in files {
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            let text = std::fs::read_to_string(&p).unwrap();
            // The code, not its tests (this one names what it looks for).
            let text = text.split("#[cfg(test)]").next().unwrap();
            if name != "client.rs" {
                for made in [
                    "reqwest::blocking::Client::",
                    "reqwest::Client::",
                    "ClientBuilder",
                ] {
                    assert!(
                        !text.contains(made),
                        "{name} makes its own HTTP client ({made})"
                    );
                }
            }
            if name != "tools.rs" {
                for made in [
                    "DaemonClient::new(",
                    "DaemonClient::with_channel(",
                    "DaemonClient::managed(",
                ] {
                    assert!(!text.contains(made), "{name} makes a DaemonClient ({made})");
                }
            }
        }
    }
}

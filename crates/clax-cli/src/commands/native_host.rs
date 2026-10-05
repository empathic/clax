//! `clax native-host` (spec 2026-10-05-chrome-overlay-design §9.1): the
//! Chrome native messaging host that pairs the Clax extension with the
//! daemon. Chrome runs it once per message, with the caller's origin as the
//! first argument; it reads one length-prefixed JSON message from stdin,
//! writes one reply to stdout, and exits. Nothing else ever goes to stdout:
//! one line per run goes to `<home>/logs/native-host.log`, and the
//! credential travels only in the reply.

use crate::client::Client;
use clax_core::Home;
use clax_core::extension::{extension_id_in_effect, extension_origin, is_credential};
use serde_json::{Value, json};
use std::io::{Read, Write};

/// The largest message read or written.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// Past this size the log is rotated to `native-host.log.1` before the next append.
const LOG_MAX_BYTES: u64 = 1 << 20;

#[derive(clap::Args)]
pub struct Args {
    /// The caller's origin, as Chrome passes it (`chrome-extension://<ID>/`).
    pub origin: Option<String>,
    /// Further arguments Chrome may pass; ignored.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub rest: Vec<String>,
}

/// Why a message got an `error` reply: `code` is the reply's `code`.
#[derive(Debug)]
pub struct HostError {
    pub code: &'static str,
    pub message: String,
}

fn bad(message: impl Into<String>) -> HostError {
    HostError {
        code: "bad_request",
        message: message.into(),
    }
}

/// Reads one message: a 32-bit native-endian length, then that many bytes
/// of JSON. A length past [`MAX_MESSAGE`], a short read or bytes that are
/// not JSON are `bad_request`.
pub fn read_message(r: &mut impl Read) -> Result<Value, HostError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)
        .map_err(|e| bad(format!("no message: {e}")))?;
    let n = u32::from_ne_bytes(len) as usize;
    if n > MAX_MESSAGE {
        return Err(bad(format!("a message is at most {MAX_MESSAGE} bytes")));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)
        .map_err(|e| bad(format!("short message: {e}")))?;
    serde_json::from_slice(&buf).map_err(|e| bad(format!("not JSON: {e}")))
}

/// Writes one message in the same framing and flushes it; a message past
/// [`MAX_MESSAGE`] is an error and nothing is written.
pub fn write_message(w: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(v).map_err(std::io::Error::other)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(std::io::Error::other("reply too large"));
    }
    w.write_all(&(bytes.len() as u32).to_ne_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// An `error` reply.
pub fn error(code: &str, message: &str) -> Value {
    json!({"type": "error", "v": 1, "code": code, "message": message})
}

/// The reply to `msg`, checked in this order: a message with no integer
/// `v` is `bad_request`; a `v` other than 1 is `unsupported_version`; a
/// `type` other than `pair` is `bad_request`; `pair` runs `pair`, whose
/// error, or panic, is an `error` reply.
pub fn answer(msg: &Value, pair: impl FnOnce() -> Result<Value, HostError>) -> Value {
    let Some(v) = msg.get("v").and_then(Value::as_u64) else {
        return error("bad_request", "a message carries its protocol version, v");
    };
    if v != 1 {
        return error(
            "unsupported_version",
            "this clax speaks version 1 of the pairing protocol",
        );
    }
    if msg["type"] != "pair" {
        return error("bad_request", "unknown message type");
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(pair)) {
        Ok(Ok(reply)) => reply,
        Ok(Err(e)) => error(e.code, &e.message),
        Err(_) => error(
            "daemon_unavailable",
            "clax native-host failed while pairing",
        ),
    }
}

/// The `paired` reply for a daemon on `port` whose mint answered `mint`;
/// `daemon_unavailable` when the answer holds no credential. `daemon` is
/// always `http://localhost:<port>`, whatever address the daemon binds,
/// since the extension talks to it from this machine only.
fn paired(port: u16, mint: &Value) -> Result<Value, HostError> {
    let credential = mint["credential"].as_str().unwrap_or_default();
    if !is_credential(credential) {
        return Err(HostError {
            code: "daemon_unavailable",
            message: "the clax daemon answered the pairing without a credential".into(),
        });
    }
    Ok(json!({
        "type": "paired",
        "v": 1,
        "daemon": format!("http://localhost:{port}"),
        "credential": credential,
        "clax_version": env!("CARGO_PKG_VERSION"),
        "viewer": mint["viewer"],
    }))
}

/// The error for a mint that failed with `e`. `probe` is the status of an
/// unauthenticated `GET /api/extension`: 404 means the daemon, of version
/// `daemon_version`, predates the extension (a held or failed upgrade kept it).
fn mint_failed(e: anyhow::Error, probe: Option<u16>, daemon_version: &str) -> HostError {
    let message = if probe == Some(404) {
        format!(
            "the running clax daemon v{daemon_version} predates the Chrome extension; run `clax stop` and try again"
        )
    } else {
        format!("pairing failed: {e:#}")
    };
    HostError {
        code: "daemon_unavailable",
        message,
    }
}

/// Ensures a daemon of this version, as `clax mcp` does (discovering it,
/// replacing an older one or starting one), and mints a credential with the
/// token.
fn pair(cli: &crate::Cli, home: &Home) -> Result<Value, HostError> {
    let port = cli.port_for(home).map_err(|e| HostError {
        code: "daemon_unavailable",
        message: format!("{e:#}"),
    })?;
    let c = Client::connect_matching_version(home, port).map_err(|e| HostError {
        code: "daemon_unavailable",
        message: format!("{e:#}; see {}", home.log_path().display()),
    })?;
    let mint = c
        .post(
            "/api/extension/credentials",
            &json!({"extension_id": extension_id_in_effect(home.root())}),
        )
        .map_err(|e| mint_failed(e, c.http_status("/api/extension"), &c.info.version))?;
    paired(c.info.port, &mint)
}

/// The log line for one run: the outcome's code, and an error's message.
/// Never the credential.
fn log_line(reply: &Value) -> String {
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    match reply["type"].as_str() {
        Some("paired") => format!("{at} native-host paired daemon={}", reply["daemon"]),
        _ => format!(
            "{at} native-host error code={} message={}",
            reply["code"], reply["message"]
        ),
    }
}

/// Appends `line` to `<home>/logs/native-host.log`, rotating it past
/// [`LOG_MAX_BYTES`]. Nothing is logged while `<home>/logs` does not exist:
/// the host makes no directories (a daemon it starts makes them). Errors are
/// ignored: the reply matters, the log does not.
fn log(home: &Home, line: &str) {
    let _ = (|| -> std::io::Result<()> {
        let dir = home.root().join("logs");
        if !dir.is_dir() {
            return Ok(());
        }
        let path = dir.join("native-host.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
            std::fs::rename(&path, path.with_extension("log.1"))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(f, "{line}")
    })();
}

/// Writes `reply` as one message to `w`; a reply too large to send is
/// replaced by an `error` reply with the same code (`bad_request` for a
/// non-error reply) and a cut message, so exactly one message is written.
fn send(w: &mut impl Write, reply: &Value) -> std::io::Result<()> {
    let fits = serde_json::to_vec(reply).is_ok_and(|b| b.len() <= MAX_MESSAGE);
    match fits {
        true => write_message(w, reply),
        false => {
            let code = reply["code"].as_str().unwrap_or("bad_request");
            let message: String = reply["message"]
                .as_str()
                .unwrap_or("the reply was too large to send")
                .chars()
                .take(1024)
                .collect();
            write_message(w, &error(code, &message))
        }
    }
}

/// Writes `reply` as the one message on stdout.
fn reply(reply: &Value) -> anyhow::Result<()> {
    send(&mut std::io::stdout().lock(), reply)?;
    Ok(())
}

/// Answers when the command line does not parse: one `bad_request` reply,
/// then exit 1.
pub fn run_unparsed(why: &str) -> ! {
    let _ = reply(&error("bad_request", why.trim()));
    std::process::exit(1);
}

/// Answers when there is no Clax home to pair with (neither `CLAX_HOME` nor
/// `HOME` set): one `daemon_unavailable` reply, then exit 1.
pub fn run_without_home(why: &str) -> ! {
    let _ = reply(&error(
        "daemon_unavailable",
        &format!("no Clax home: {why}"),
    ));
    std::process::exit(1);
}

/// Checks the origin, reads one message and writes one reply. A wrong or
/// missing origin gets a `wrong_origin` reply and an error (exit 1).
pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let ours = format!(
        "{}/",
        extension_origin(&extension_id_in_effect(home.root()))
    );
    if a.origin.as_deref() != Some(ours.as_str()) {
        let r = error(
            "wrong_origin",
            "clax native-host answers only the Clax extension of this Clax home",
        );
        log(home, &log_line(&r));
        reply(&r)?;
        anyhow::bail!("clax native-host was started for another origin");
    }
    let r = match read_message(&mut std::io::stdin().lock()) {
        Ok(m) => answer(&m, || pair(cli, home)),
        Err(e) => error(e.code, &e.message),
    };
    log(home, &log_line(&r));
    reply(&r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn framed(v: &serde_json::Value) -> Vec<u8> {
        let mut out = Vec::new();
        write_message(&mut out, v).unwrap();
        out
    }

    #[test]
    fn messages_round_trip_with_a_native_endian_length() {
        let v = json!({"type": "pair", "v": 1});
        let bytes = framed(&v);
        let n = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert_eq!(n, bytes.len() - 4);
        assert_eq!(read_message(&mut bytes.as_slice()).unwrap(), v);
    }

    #[test]
    fn oversized_truncated_and_malformed_messages_are_bad_requests() {
        let mut big = ((MAX_MESSAGE + 1) as u32).to_ne_bytes().to_vec();
        big.extend(std::iter::repeat_n(b' ', 16));
        assert_eq!(
            read_message(&mut big.as_slice()).unwrap_err().code,
            "bad_request"
        );
        let mut short = 10u32.to_ne_bytes().to_vec();
        short.extend(b"{}");
        assert_eq!(
            read_message(&mut short.as_slice()).unwrap_err().code,
            "bad_request"
        );
        let mut junk = 3u32.to_ne_bytes().to_vec();
        junk.extend(b"{{{");
        assert_eq!(
            read_message(&mut junk.as_slice()).unwrap_err().code,
            "bad_request"
        );
        let empty: &[u8] = &[];
        assert_eq!(
            read_message(&mut { empty }).unwrap_err().code,
            "bad_request"
        );
    }

    #[test]
    fn a_reply_past_the_limit_is_not_written() {
        let mut out = Vec::new();
        let big = json!({"s": "x".repeat(MAX_MESSAGE)});
        assert!(write_message(&mut out, &big).is_err());
        assert!(out.is_empty());
    }

    #[test]
    fn only_pair_v1_is_answered() {
        let paired = || Ok(json!({"type": "paired"}));
        assert_eq!(
            answer(&json!({"type": "pair", "v": 1}), paired)["type"],
            "paired"
        );
        assert_eq!(
            answer(&json!({"type": "pair", "v": 2}), paired)["code"],
            "unsupported_version"
        );
        assert_eq!(
            answer(&json!({"type": "other", "v": 1}), paired)["code"],
            "bad_request"
        );
        let down = || {
            Err(HostError {
                code: "daemon_unavailable",
                message: "no".into(),
            })
        };
        let e = answer(&json!({"type": "pair", "v": 1}), down);
        assert_eq!(e["type"], "error");
        assert_eq!(e["v"], 1);
        assert_eq!(e["code"], "daemon_unavailable");
        assert_eq!(e["message"], "no");
    }

    #[test]
    fn a_message_without_a_version_is_a_bad_request() {
        let paired = || Ok(json!({"type": "paired"}));
        assert_eq!(
            answer(&json!({"type": "pair"}), paired)["code"],
            "bad_request"
        );
        assert_eq!(answer(&json!([1]), paired)["code"], "bad_request");
    }

    #[test]
    fn a_panic_while_pairing_is_an_error_reply() {
        let e = answer(&json!({"type": "pair", "v": 1}), || panic!("boom"));
        assert_eq!(e["type"], "error");
        assert_eq!(e["code"], "daemon_unavailable");
    }

    #[test]
    fn a_mint_without_a_credential_is_not_paired() {
        let ok = json!({"credential": format!("cxe_{}", "A".repeat(43)), "viewer": {"public_id": "u_1"}});
        let p = paired(7, &ok).unwrap();
        assert_eq!(p["type"], "paired");
        assert_eq!(p["daemon"], "http://localhost:7");
        assert_eq!(p["viewer"]["public_id"], "u_1");
        for bad in [
            json!({}),
            json!({"credential": null}),
            json!({"credential": "nope"}),
        ] {
            assert_eq!(paired(7, &bad).unwrap_err().code, "daemon_unavailable");
        }
    }

    #[test]
    fn a_reply_too_large_to_send_becomes_an_error_reply() {
        let mut out = Vec::new();
        let big = error("daemon_unavailable", &"x".repeat(MAX_MESSAGE));
        send(&mut out, &big).unwrap();
        let v = read_message(&mut out.as_slice()).unwrap();
        assert_eq!(v["type"], "error");
        assert_eq!(v["code"], "daemon_unavailable");
        assert!(out.len() <= MAX_MESSAGE + 4);
    }

    #[test]
    fn a_failed_mint_from_an_older_daemon_names_it() {
        let e = mint_failed(anyhow::anyhow!("error: request failed"), Some(404), "0.0.1");
        assert_eq!(e.code, "daemon_unavailable");
        assert!(e.message.contains("v0.0.1"), "{}", e.message);
        assert!(e.message.contains("clax stop"), "{}", e.message);
        let e = mint_failed(anyhow::anyhow!("boom"), Some(401), "9.9.9");
        assert!(e.message.contains("boom"), "{}", e.message);
    }

    #[test]
    fn the_log_line_names_the_outcome_but_not_the_credential() {
        let paired =
            json!({"type": "paired", "daemon": "http://localhost:1", "credential": "cxe_secret"});
        let line = log_line(&paired);
        assert!(line.contains("paired"), "{line}");
        assert!(!line.contains("cxe_secret"), "{line}");
        let line = log_line(&error("bad_request", "short\nmessage"));
        assert!(line.contains("code=\"bad_request\""), "{line}");
        assert!(!line.contains('\n'), "{line}");
    }
}

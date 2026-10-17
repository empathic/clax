//! `clax connect` (spec §16, §3.1, §3.2, §22): connects this machine to
//! another machine's Clax daemon as a person's machine, shows the
//! connection, moves it to a new address, or ends it.
//!
//! With an invite (`http://<IP>:<port>/i/<code>`) the grant is issued at
//! once. With a daemon's address alone the request waits for the owner,
//! who approves it in their browser with the code this prints; the request
//! is kept in `remote-pending.json` while it waits, so a connect that is
//! killed resumes on the next run against the same address and one left
//! behind is cancelled by `--off` or by connecting elsewhere; Ctrl-C and
//! the ten-minute limit cancel it on the daemon at once, freeing the key
//! and the peer's request slot. Connected, it writes `remote.json` and
//! `remote.key` (0600), asks the person whether the owner's comments and
//! pages may reach their agents everywhere (M15), and opens the browser
//! link that makes their browser theirs (spec §6.5).
//!
//! No token, key or code is written to a log; the user code and the
//! browser link are printed for the person, as they must be.

use crate::client::Client;
use crate::remote::{Refused, Unreachable};
use crate::term::clean_line;
use clax_core::Home;
use clax_core::remote::{
    Credential, DaemonUrl, Mode, PendingConnect, RemoteConfig, Signer, load_key, save_key,
};
use clax_core::secret::Secret;
use clax_core::sig::KeyPair;
use reqwest::Method;
use serde_json::{Value, json};
use std::io::{BufRead, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long an ad-hoc request may wait for the owner (spec §6.3: a pending
/// request lapses after ten minutes).
const WAIT_LIMIT: Duration = Duration::from_secs(600);

#[derive(clap::Args)]
pub struct Args {
    /// An invite (`http://<IP>:<port>/i/<code>`, as the owner's People page
    /// shows it), or the owner's daemon (`http://<IP>:<port>`) to ask the
    /// owner, who approves the request with the code this prints.
    #[arg(value_name = "URL")]
    pub target: Option<String>,
    /// Show the connection: the daemon, the person, the key, whether it
    /// answers and how far its clock is from this machine's.
    #[arg(long, conflicts_with_all = ["target", "off", "new_url"])]
    pub status: bool,
    /// Disconnect: end this machine's grant on the daemon and remove
    /// `remote.json` and `remote.key` (or cancel a request still waiting).
    #[arg(long, conflicts_with_all = ["target", "new_url"])]
    pub off: bool,
    /// The daemon's new address (`http://<IP>:<port>`), when the owner's
    /// machine got another one: the grant and key are kept.
    #[arg(long = "url", value_name = "URL", conflicts_with = "target")]
    pub new_url: Option<String>,
    /// The name this machine gives the owner (default: its host name).
    #[arg(long)]
    pub name: Option<String>,
    /// Print the browser link instead of opening it.
    #[arg(long)]
    pub no_browser: bool,
    /// Answer yes to letting the owner's comments and pages reach your
    /// agents everywhere.
    #[arg(long)]
    pub yes: bool,
    /// Milliseconds between polls while a request waits (default: the
    /// daemon's `wait`).
    #[arg(long, hide = true, default_value_t = 0)]
    pub poll_interval_ms: u64,
}

pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    if a.status {
        return status(cli, home);
    }
    // Connecting a machine, moving or ending its connection, and answering
    // for the person about the owner are the person's acts (M15, M20):
    // never an agent's.
    if crate::channel::from_process() {
        return Err(crate::client::ForPeople {
            line: "This is for people, not agents: connecting this machine to a Clax is yours to do; run `clax connect` in your own terminal.".into(),
        }
        .into());
    }
    if a.off {
        return off(cli, home);
    }
    if let Some(u) = &a.new_url {
        return set_url(cli, home, u);
    }
    match &a.target {
        Some(t) => connect(cli, home, a, t),
        None => anyhow::bail!(
            "give an invite (http://<IP>:<port>/i/<code>) or the owner's daemon (http://<IP>:<port>); or --status, --off, --url"
        ),
    }
}

/// The name this machine gives the owner: `--name`, else its host name.
fn client_name(a: &Args) -> String {
    a.name.clone().unwrap_or_else(|| {
        nix::unistd::gethostname()
            .ok()
            .and_then(|h| h.into_string().ok())
            .map(|h| h.trim_end_matches(".local").to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "this machine".to_string())
    })
}

/// The grant request's body for `key`, named `name`, redeeming `invite`
/// when given and otherwise asking the owner with a user code.
fn request_body(key: &KeyPair, name: &str, invite: Option<&str>) -> Value {
    let mut body = json!({
        "client": {
            "key": {"proof": "clax-ed25519", "jwk": {"kty": "OKP", "crv": "Ed25519",
                    "x": clax_core::sig::encode_public(&key.public())}},
            "display": {"name": name},
        },
    });
    match invite {
        Some(code) => body["clax"] = json!({"invite": code}),
        None => body["interact"] = json!({"start": ["user_code"]}),
    }
    body
}

/// `code: message` of a refusal's body, for the person, made safe to
/// print.
fn refusal(status: u16, v: &Value) -> anyhow::Error {
    anyhow::anyhow!(
        "{}: {}",
        clean_line(v["error"]["code"].as_str().unwrap_or("error")),
        clean_line(&v["error"]["message"].as_str().map_or_else(
            || format!("the daemon answered HTTP {status}"),
            str::to_string
        ))
    )
}

/// `link` when it is the browser link of the daemon at `base` (spec §6.5):
/// exactly `<base>/connect/b/<code>`, the code in base64url. Anything else
/// a daemon sends is neither opened nor printed.
fn browser_link(base: &str, link: &str) -> Option<String> {
    let code = link.strip_prefix(base)?.strip_prefix("/connect/b/")?;
    let ok = (1..=128).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then(|| link.to_string())
}

/// Removes the files of an unfinished connect: the pending request and
/// the key no grant holds.
fn forget_pending(home: &Home) {
    let _ = std::fs::remove_file(home.remote_pending());
    if !home.remote_json().exists() {
        let _ = std::fs::remove_file(home.remote_key());
    }
}

/// Cancels the request `p` names on its daemon (best effort: one that
/// lapsed or cannot be reached ends by itself within ten minutes) and
/// forgets it.
fn cancel(home: &Home, p: &PendingConnect) {
    if let Ok(key) = load_key(home)
        && let Ok(signer) = Signer::new(key, &p.url)
    {
        let c = Client::for_signer(Arc::new(signer));
        let _ = c.signed_call(
            Method::DELETE,
            &format!("/api/grants/{}/continue", p.grant_id),
            None,
            Credential::Gnap(&p.continue_token),
        );
    }
    forget_pending(home);
}

fn connect(cli: &crate::Cli, home: &Home, a: &Args, target: &str) -> anyhow::Result<()> {
    let url = DaemonUrl::parse(target).map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Ok(Some(cfg)) = RemoteConfig::load(home) {
        anyhow::bail!(
            "already connected to {} as {}: run `clax connect --off` first",
            cfg.url,
            cfg.person_name.as_deref().unwrap_or("a person")
        );
    } else if home.remote_json().exists() {
        anyhow::bail!(
            "{} exists but cannot be read: run `clax connect --off` first",
            home.remote_json().display()
        );
    }
    if let Some(p) = PendingConnect::load(home) {
        if url.invite.is_none() && p.url == url.base {
            return resume(cli, home, a, p);
        }
        // Asked somewhere else, or now with an invite: the old request goes.
        cancel(home, &p);
    }
    let name = client_name(a);
    let key = KeyPair::generate();
    save_key(home, &key)?;
    let signer = Arc::new(Signer::new(key, &url.base).map_err(|e| anyhow::anyhow!("{e}"))?);
    let c = Client::for_signer(signer.clone());
    let body = request_body(signer.key(), &name, url.invite.as_deref());
    let answer = c.signed_call(Method::POST, "/api/grants", Some(&body), Credential::None);
    let (status, v) = match answer {
        Ok(x) => x,
        Err(e) => {
            forget_pending(home);
            return Err(e);
        }
    };
    match status {
        200 => {
            say(cli, &format!("key {}", signer.fingerprint()));
            finish(cli, home, a, &c, &url.base, &name, &v)
        }
        201 => {
            let p = PendingConnect {
                url: url.base.clone(),
                grant_id: grant_of(&v)?,
                continue_token: Secret::new(
                    v["continue"]["access_token"]["value"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ),
                user_code: match v["interact"]["user_code"]
                    .as_str()
                    .and_then(clax_core::secret::canonical_user_code)
                {
                    Some(c) => format!("{}-{}", &c[..4], &c[4..]),
                    None => {
                        forget_pending(home);
                        anyhow::bail!("the daemon sent no usable user code: it is not a Clax");
                    }
                },
                client_name: name,
                requested_at: chrono::Utc::now().to_rfc3339(),
            };
            p.save(home)?;
            wait(cli, home, a, &c, p)
        }
        _ => {
            forget_pending(home);
            Err(refusal(status, &v))
        }
    }
}

/// The grant ID in a response's `continue.uri`.
fn grant_of(v: &Value) -> anyhow::Result<String> {
    v["continue"]["uri"]
        .as_str()
        .and_then(|u| u.strip_prefix("/api/grants/"))
        .and_then(|r| r.strip_suffix("/continue"))
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("the daemon's answer names no grant"))
}

/// A line for the person (stderr under `--json`, which keeps stdout for
/// the one JSON object).
fn say(cli: &crate::Cli, line: &str) {
    if cli.json {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
    let _ = std::io::stdout().flush();
}

/// Resumes the request a killed connect left in `remote-pending.json`.
fn resume(cli: &crate::Cli, home: &Home, a: &Args, p: PendingConnect) -> anyhow::Result<()> {
    let signer = match load_key(home)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .and_then(|k| Signer::new(k, &p.url).map_err(|e| anyhow::anyhow!("{e}")))
    {
        Ok(s) => s,
        Err(_) => {
            // The key is gone: the request can never be finished.
            forget_pending(home);
            anyhow::bail!(
                "the request this machine left waiting lost its key: run `clax connect {}` again",
                p.url
            );
        }
    };
    let c = Client::for_signer(Arc::new(signer));
    say(
        cli,
        &format!("resuming the request made at {}", p.requested_at),
    );
    wait(cli, home, a, &c, p)
}

/// Listens for Ctrl-C from now on: while `waiting` holds, Ctrl-C sets the
/// returned flag (the wait cancels the request); once it is cleared,
/// Ctrl-C ends the process as it would without a listener. The listener
/// is in place when this returns.
fn interrupt_flag(waiting: Arc<AtomicBool>) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let f = flag.clone();
    let (ready, listening) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            let _ = ready.send(());
            return;
        };
        let signal = {
            let _g = rt.enter();
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        };
        let _ = ready.send(());
        let Ok(mut s) = signal else {
            return;
        };
        while rt.block_on(s.recv()).is_some() {
            if !waiting.load(Ordering::SeqCst) {
                std::process::exit(130);
            }
            f.store(true, Ordering::SeqCst);
        }
    });
    let _ = listening.recv();
    flag
}

/// Waits for the owner to approve request `p`, polling its continuation;
/// connected, finishes as an invite does. Ctrl-C and [`WAIT_LIMIT`] cancel
/// the request on the daemon; a denial, a lapse or tokens issued to a
/// response that never arrived forget it.
fn wait(
    cli: &crate::Cli,
    home: &Home,
    a: &Args,
    c: &Client,
    p: PendingConnect,
) -> anyhow::Result<()> {
    let waiting = Arc::new(AtomicBool::new(true));
    let interrupted = interrupt_flag(waiting.clone());
    let (url, name) = (p.url.clone(), p.client_name.clone());
    let issued = wait_for(cli, home, a, c, p, &interrupted);
    // From here Ctrl-C ends the process, at the question about the owner too.
    waiting.store(false, Ordering::SeqCst);
    finish(cli, home, a, c, &url, &name, &issued?)
}

/// [`wait`]'s polling, until the owner answers (the answer that issues the
/// tokens), `interrupted` is set or the time is up.
fn wait_for(
    cli: &crate::Cli,
    home: &Home,
    a: &Args,
    c: &Client,
    p: PendingConnect,
    interrupted: &AtomicBool,
) -> anyhow::Result<Value> {
    let signer = c.signer().expect("a signing client").clone();
    say(cli, &format!("key {}", signer.fingerprint()));
    say(
        cli,
        &format!(
            "Ask the owner of {} to approve request {}: waiting up to 10 minutes (Ctrl-C cancels it)",
            p.url, p.user_code
        ),
    );
    let since = chrono::DateTime::parse_from_rfc3339(&p.requested_at)
        .map(|t| t.timestamp())
        .unwrap_or_else(|_| chrono::Utc::now().timestamp());
    let left = WAIT_LIMIT.saturating_sub(Duration::from_secs(
        (chrono::Utc::now().timestamp() - since).max(0) as u64,
    ));
    let deadline = Instant::now() + left;
    let mut every = Duration::from_millis(a.poll_interval_ms);
    if every.is_zero() {
        every = Duration::from_secs(clax_store::people::CONTINUE_WAIT_SECS as u64);
    }
    let path = format!("/api/grants/{}/continue", p.grant_id);
    let mut warned = false;
    loop {
        // Waits `every`, watching for Ctrl-C.
        let until = Instant::now() + every;
        while Instant::now() < until && !interrupted.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20).min(every));
        }
        if interrupted.load(Ordering::SeqCst) {
            cancel(home, &p);
            anyhow::bail!("interrupted: request {} cancelled", p.user_code);
        }
        if Instant::now() >= deadline {
            cancel(home, &p);
            anyhow::bail!(
                "the owner did not answer within 10 minutes: request {} cancelled; run `clax connect {}` to ask again",
                p.user_code,
                p.url
            );
        }
        let lost = || {
            forget_pending(home);
            anyhow::anyhow!(
                "request {}'s tokens were issued, but the answer never reached this machine: ask the owner to revoke the connection from {} in People, then run `clax connect {}` again",
                p.user_code,
                p.client_name,
                p.url
            )
        };
        match c.signed_call(
            Method::POST,
            &path,
            None,
            Credential::Gnap(&p.continue_token),
        ) {
            Ok((200, v)) if v.get("access_token").is_some() => {
                // `finish` removes remote-pending.json once remote.json is
                // saved, so the tokens are never only in memory.
                return Ok(v);
            }
            Ok((200, _)) => {}
            Ok((400, v)) if v["error"]["code"] == "too_fast" => {}
            Ok((403, _)) => {
                forget_pending(home);
                anyhow::bail!(
                    "the owner declined request {}, or it lapsed: run `clax connect {}` to ask again",
                    p.user_code,
                    p.url
                );
            }
            Ok((409, v)) if v["error"]["code"] == "already_issued" => return Err(lost()),
            Ok((status, v)) => return Err(refusal(status, &v)),
            Err(e) => match e.downcast_ref::<Refused>() {
                Some(r) if r.code == "invalid_token" => return Err(lost()),
                Some(_) => return Err(e),
                None if e.downcast_ref::<Unreachable>().is_some() => {
                    if !warned {
                        say(cli, &format!("{e}; still waiting"));
                        warned = true;
                    }
                }
                None => return Err(e),
            },
        }
    }
}

/// Connected: writes `remote.json`, says so, asks about the owner (M15)
/// and opens the browser link.
fn finish(
    cli: &crate::Cli,
    home: &Home,
    a: &Args,
    c: &Client,
    base: &str,
    name: &str,
    v: &Value,
) -> anyhow::Result<()> {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_string);
    let cfg = RemoteConfig {
        url: base.to_string(),
        grant_id: grant_of(v)?,
        access_token: Secret::new(s("/access_token/value").unwrap_or_default()),
        manage_token: Secret::new(s("/access_token/manage/access_token/value").unwrap_or_default()),
        continue_token: Secret::new(s("/continue/access_token/value").unwrap_or_default()),
        person_name: s("/clax/person/display_name"),
        person_id: s("/clax/person/public_id"),
        owner_name: s("/clax/owner/display_name"),
        own_machine: v.pointer("/clax/own_machine") == Some(&Value::Bool(true)),
        client_name: name.to_string(),
        connected_at: chrono::Utc::now().to_rfc3339(),
    };
    cfg.save(home)?;
    let _ = std::fs::remove_file(home.remote_pending());
    let owner = cfg
        .owner_name
        .as_deref()
        .map(crate::term::clean_line)
        .map_or_else(|| "the owner's".to_string(), |o| format!("{o}'s"));
    let person = cfg
        .person_name
        .as_deref()
        .map(crate::term::clean_line)
        .unwrap_or_else(|| "a person".to_string());
    if cfg.own_machine {
        say(
            cli,
            &format!("Connected to your own Clax at {base}: this machine's agents act as yours."),
        );
    } else {
        say(cli, &format!("Connected to {owner} Clax as {person}."));
    }
    let approved = if cfg.own_machine {
        None
    } else {
        Some(ask_about_owner(cli, a, c, &cfg, &owner)?)
    };
    let sent = s("/clax/browser_link");
    let link = sent.as_deref().and_then(|l| browser_link(base, l));
    if sent.is_some() && link.is_none() {
        say(
            cli,
            "The daemon sent no usable browser link: open People in your browser on the owner's Clax to use it as yourself.",
        );
    }
    if let Some(link) = &link {
        let opened = !a.no_browser && !cli.json && clax_mcp::tools::open_in_browser(link);
        if opened {
            say(
                cli,
                &format!("Opened your browser as {person} (the link works once, for 10 minutes)."),
            );
        } else {
            say(
                cli,
                &format!(
                    "Open {link} in your browser within 10 minutes to use Clax there as {person} (it works once)."
                ),
            );
        }
    }
    if cli.json {
        println!(
            "{}",
            json!({
                "connected": true,
                "url": cfg.url,
                "grant_id": cfg.grant_id,
                "person": cfg.person_name,
                "owner": cfg.owner_name,
                "own_machine": cfg.own_machine,
                "key": c.signer().map(|s| s.fingerprint()),
                "owner_approved": approved,
                "browser_link": link,
            })
        );
    }
    Ok(())
}

/// Asks "Let <owner>'s comments and pages reach your agents everywhere?
/// [Y/n]" (M15): `--yes` answers yes; off a terminal, with no `--yes`, the
/// question goes unanswered and nothing is approved. A yes is recorded on
/// the daemon as the person's approval of the owner, everywhere.
fn ask_about_owner(
    cli: &crate::Cli,
    a: &Args,
    c: &Client,
    cfg: &RemoteConfig,
    owner: &str,
) -> anyhow::Result<bool> {
    let question = format!("Let {owner} comments and pages reach your agents everywhere? [Y/n]");
    let yes = if a.yes {
        say(cli, &format!("{question} yes (--yes)"));
        true
    } else if std::io::stdin().is_terminal() && !cli.json {
        print!("{question} ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        let l = line.trim().to_lowercase();
        l.is_empty() || l == "y" || l == "yes"
    } else {
        say(
            cli,
            &format!(
                "{question} not answered (no terminal; pass --yes): allow {owner} content in People when you want it"
            ),
        );
        false
    };
    if !yes {
        return Ok(false);
    }
    match c.signed_call(
        Method::POST,
        &format!("/api/grants/{}/continue", cfg.grant_id),
        Some(&json!({"clax": {"approve_owner": true}})),
        Credential::Gnap(&cfg.continue_token),
    ) {
        Ok((200, _)) => {
            say(cli, &format!("Your agents see {owner} comments and pages."));
            Ok(true)
        }
        Ok((status, v)) => {
            say(
                cli,
                &format!(
                    "could not record that ({}); allow {owner} content in People instead",
                    refusal(status, &v)
                ),
            );
            Ok(false)
        }
        Err(e) => {
            say(
                cli,
                &format!("could not record that ({e}); allow {owner} content in People instead"),
            );
            Ok(false)
        }
    }
}

/// The connection as `clax connect --status` and `clax status` show it:
/// `None` when this machine is connected to no remote daemon (a request
/// still waiting is named in `pending`). `honour_env`: `CLAX_REMOTE=0`
/// counts (`clax status`); `clax connect`'s own subcommands read
/// `remote.json` whatever it says.
///
/// # Errors
/// `remote.json` or the key cannot be used.
pub fn remote_status(home: &Home, honour_env: bool) -> anyhow::Result<Option<(Value, Vec<Value>)>> {
    let env = std::env::var(clax_core::remote::REMOTE_ENV).ok();
    let mode = Mode::with_env(home, env.as_deref().filter(|_| honour_env));
    let r = match mode {
        Mode::Local => return Ok(None),
        Mode::Broken(e) => anyhow::bail!("{e}"),
        Mode::Remote(r) => r,
    };
    let c = Client::for_remote(r.clone());
    let cfg = &r.config;
    let mut v = json!({
        "url": cfg.url,
        "person": cfg.person_name,
        "owner": cfg.owner_name,
        "own_machine": cfg.own_machine,
        "client_name": cfg.client_name,
        "grant_id": cfg.grant_id,
        "key": r.signer.fingerprint(),
        "connected_at": cfg.connected_at,
    });
    let agents = match c.probe_remote() {
        Ok(p) => {
            v["reachable"] = json!(true);
            v["clock_offset_s"] = json!(p.offset_s);
            p.agents
        }
        Err(e) => {
            v["reachable"] = json!(false);
            v["error"] = json!(format!("{e:#}"));
            Vec::new()
        }
    };
    Ok(Some((v, agents)))
}

/// The lines of [`remote_status`]'s connection, for people.
pub fn status_lines(v: &Value) -> Vec<String> {
    let t = |k: &str| v[k].as_str().map(crate::term::clean_line);
    let who = if v["own_machine"] == true {
        "a machine of your own".to_string()
    } else {
        format!(
            "as {}{}",
            t("person").unwrap_or_else(|| "a person".into()),
            t("owner").map_or_else(String::new, |o| format!(" ({o}'s Clax)"))
        )
    };
    let mut lines = vec![format!(
        "connected to {} {who}: this machine is \"{}\", key {}",
        v["url"].as_str().unwrap_or_default(),
        t("client_name").unwrap_or_default(),
        v["key"].as_str().unwrap_or_default()
    )];
    if v["reachable"] == true {
        let offset = v["clock_offset_s"].as_i64();
        let skew = match offset {
            Some(o) if o.abs() > clax_core::sig::MAX_SKEW_S => format!(
                "; its clock is {o:+} s from this machine's, beyond the {} s signed requests allow: set this machine's clock",
                clax_core::sig::MAX_SKEW_S
            ),
            Some(o) => format!("; clock offset {o:+} s"),
            None => String::new(),
        };
        lines.push(format!("reachable{skew}"));
    } else {
        lines.push(format!(
            "not reachable: {} (if the owner's machine has a new address: `clax connect --url http://<IP>:<port>`)",
            v["error"].as_str().unwrap_or("no answer")
        ));
    }
    lines
}

fn status(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    match remote_status(home, false)? {
        Some((v, _)) => super::print(cli, json!({"remote": v}), |j| {
            status_lines(&j["remote"]).join("\n")
        }),
        None => match PendingConnect::load(home) {
            Some(p) => super::print(
                cli,
                json!({"remote": null, "pending": {"url": p.url, "user_code": p.user_code, "requested_at": p.requested_at}}),
                |_| {
                    format!(
                        "not connected: request {} to {} waits for the owner (`clax connect {}` resumes it, `clax connect --off` cancels it)",
                        p.user_code, p.url, p.url
                    )
                },
            ),
            None => super::print(cli, json!({"remote": null}), |_| {
                "not connected to a remote Clax".to_string()
            }),
        },
    }
    Ok(())
}

/// Removes everything a connection keeps here.
fn forget_connection(home: &Home) {
    let _ = std::fs::remove_file(home.remote_json());
    let _ = std::fs::remove_file(home.remote_key());
    let _ = std::fs::remove_file(home.remote_pending());
    let _ = std::fs::remove_dir_all(home.remote_cache());
    let _ = std::fs::remove_dir_all(home.remote_sessions());
}

fn off(cli: &crate::Cli, home: &Home) -> anyhow::Result<()> {
    let mut said = Vec::new();
    if let Some(p) = PendingConnect::load(home) {
        cancel(home, &p);
        said.push(format!("cancelled request {} to {}", p.user_code, p.url));
    }
    match RemoteConfig::load(home) {
        Ok(Some(cfg)) => {
            let ended = match load_key(home) {
                Ok(key) => {
                    let signer = Signer::new(key, &cfg.url).map_err(|e| anyhow::anyhow!("{e}"))?;
                    let c = Client::for_signer(Arc::new(signer));
                    c.signed_call(
                        Method::DELETE,
                        &format!("/api/grants/{}/continue", cfg.grant_id),
                        None,
                        Credential::Gnap(&cfg.continue_token),
                    )
                }
                Err(e) => Err(anyhow::anyhow!("{e}")),
            };
            forget_connection(home);
            said.push(match ended {
                Ok((s, _)) if (200..300).contains(&s) => format!(
                    "Disconnected from {}: the daemon ended this machine's grant",
                    cfg.url
                ),
                Ok((401 | 403 | 404, _)) => format!(
                    "Disconnected from {}: its grant had already ended",
                    cfg.url
                ),
                Err(e)
                    if e.downcast_ref::<Refused>().is_some_and(|r| {
                        matches!(r.code.as_str(), "invalid_token" | "grant_revoked")
                    }) =>
                {
                    format!(
                    "Disconnected from {}: its grant had already ended",
                    cfg.url
                    )
                }
                Ok((status, v)) => format!(
                    "Disconnected here; the daemon at {} answered {}: without the key the grant signs nothing, and the owner can revoke it in People",
                    cfg.url,
                    refusal(status, &v)
                ),
                Err(e) => format!(
                    "Disconnected here; {e}: without the key the grant signs nothing, and the owner can revoke it in People"
                ),
            });
        }
        Ok(None) => {}
        Err(e) => {
            forget_connection(home);
            said.push(format!("Disconnected here ({e})"));
        }
    }
    if said.is_empty() {
        said.push("this machine is not connected to a remote Clax".to_string());
    }
    super::print(cli, json!({"connected": false, "said": said}), |_| {
        said.join("\n")
    });
    Ok(())
}

fn set_url(cli: &crate::Cli, home: &Home, raw: &str) -> anyhow::Result<()> {
    let u = DaemonUrl::parse(raw).map_err(|e| anyhow::anyhow!("{e}"))?;
    if u.invite.is_some() {
        anyhow::bail!("--url takes the daemon's address (http://<IP>:<port>), not an invite");
    }
    let mut cfg = RemoteConfig::load(home)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .ok_or_else(|| {
            anyhow::anyhow!("this machine is not connected: run `clax connect <invite>`")
        })?;
    let old = std::mem::replace(&mut cfg.url, u.base);
    cfg.save(home)?;
    let (v, _) = remote_status(home, false)?
        .ok_or_else(|| anyhow::anyhow!("remote.json was removed while it was rewritten"))?;
    super::print(cli, json!({"remote": v, "previous_url": old}), |j| {
        let mut lines = vec![format!(
            "now connecting to {} (was {old}); the grant and key are kept",
            j["remote"]["url"].as_str().unwrap_or_default()
        )];
        lines.extend(status_lines(&j["remote"]));
        lines.join("\n")
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::browser_link;

    #[test]
    fn only_the_connected_daemons_browser_link_is_opened() {
        let base = "http://192.168.1.5:7480";
        let good = format!("{base}/connect/b/aZ09_-");
        assert_eq!(browser_link(base, &good), Some(good.clone()));
        for bad in [
            "file:///Applications/Evil.app".to_string(),
            "-a Calculator".to_string(),
            format!("{base}/connect/b/"),
            format!("{base}/connect/b/a/b"),
            format!("{base}/connect/b/a?x=1"),
            format!("{base}/connect/b/a b"),
            format!("{base}/people"),
            "http://192.168.1.6:7480/connect/b/abc".to_string(),
            format!("{base}.evil/connect/b/abc"),
            format!("{base}/connect/b/{}", "a".repeat(129)),
        ] {
            assert_eq!(browser_link(base, &bad), None, "{bad}");
        }
    }
}

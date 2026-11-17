//! `fake_host`: a [`FakeHost`] over a daemon's HTTP API, for tests that
//! drive a real daemon.
//!
//! ```text
//! fake_host --url <base> --token-file <path> --surface <file>
//!           --answers <json file> --stop-file <path> [--wait-ms <ms>]
//!           [--registration <id>] [--registration-file <path>]
//!           [--deliveries <path>] [--retry-for <seconds>]
//! ```
//!
//! It registers the surface in `--surface` (a `.d.ts` file, or else a
//! manifest), prints the daemon's `Registered` as one JSON line each
//! time it registers, and serves round after round, answering each
//! operation as `--answers` says (`{"<op>": <behaviour>}`,
//! [`Behaviour::from_json`]; any other operation is never answered).
//!
//! - **Its registration.** With `--registration`, it goes on as that
//!   ID. With `--registration-file`, it goes on as the ID the file
//!   holds, when the file is there, and writes its ID there each time
//!   it registers: a host process started again finds its own ID, and
//!   is sent the calls that waited for it. A registration the daemon
//!   no longer holds (`unknown_registration` from a `serve`) is
//!   registered again as itself, and afresh when the daemon does not
//!   know it either ([`FakeHost::register`]); a host that meant to go
//!   on as an ID and registered afresh says so on stderr.
//! - **A daemon that is away.** A request that got no answer at all
//!   (`transport`), or whose token was refused (`unauthorized`: a
//!   daemon that restarted has a new token), is tried again after a
//!   pause that grows from 100 ms to 5 s, with `--token-file` read
//!   again, until `--retry-for` seconds of pauses have gone by with no
//!   other answer (default 0: it exits 1 at once). It looks for
//!   `--stop-file` between tries. `--token-file` holds the token, or is
//!   a home's `daemon.json`.
//! - **What it saw.** With `--deliveries`, each delivery is appended to
//!   that file as one JSON line when it comes.
//! - **Stopping.** Before each round it looks for `--stop-file`. Once
//!   it exists, it sends the answers it owes with one last `serve` that
//!   waits for nothing (what that round delivers is kept, not
//!   answered), prints `{"applied": {...}, "deliveries": [...]}` as one
//!   JSON line and exits 0.
//!
//! Any other refusal is printed to stderr with its body, and exits 1.

use clax_surface::wire::{CODE_UNKNOWN_REGISTRATION, Delivery, Notation, Registered};
use clax_testkit::fake_host::{Behaviour, FakeHost, HostTransport, HttpTransport, Served};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The first pause after a request that got no answer, and the longest.
const FIRST_PAUSE_MS: u64 = 100;
const LONGEST_PAUSE_MS: u64 = 5_000;

/// The command line, read.
#[derive(Debug, PartialEq)]
struct Options {
    url: String,
    token_file: String,
    surface: String,
    answers: String,
    stop_file: PathBuf,
    wait_ms: u64,
    registration: Option<String>,
    registration_file: Option<PathBuf>,
    deliveries: Option<PathBuf>,
    retry_for_ms: u64,
}

/// Reads `--<name> <value>` pairs: each flag once, none unknown.
fn parse(given: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut args = BTreeMap::new();
    let mut given = given.into_iter();
    while let Some(flag) = given.next() {
        let (Some(name), Some(value)) = (flag.strip_prefix("--"), given.next()) else {
            return Err(format!("expected --<name> <value>, got {flag}"));
        };
        if args.insert(name.to_string(), value).is_some() {
            return Err(format!("--{name} is given twice"));
        }
    }
    let mut take = |name: &str| args.remove(name);
    let required = |name: &str, v: Option<String>| v.ok_or(format!("--{name} is required"));
    let number = |name: &str, v: Option<String>| {
        v.map(|v| v.parse::<u64>().map_err(|e| format!("--{name}: {e}")))
            .transpose()
    };
    let options = Options {
        url: required("url", take("url"))?,
        token_file: required("token-file", take("token-file"))?,
        surface: required("surface", take("surface"))?,
        answers: required("answers", take("answers"))?,
        stop_file: required("stop-file", take("stop-file"))?.into(),
        wait_ms: number("wait-ms", take("wait-ms"))?.unwrap_or(1_000),
        registration: take("registration"),
        registration_file: take("registration-file").map(PathBuf::from),
        deliveries: take("deliveries").map(PathBuf::from),
        retry_for_ms: number("retry-for", take("retry-for"))?
            .unwrap_or(0)
            .saturating_mul(1_000),
    };
    match args.keys().next() {
        Some(unknown) => Err(format!("--{unknown} is not a flag of fake_host")),
        None => Ok(options),
    }
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

fn at(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

/// The ID a registration file holds: none when it is not there or empty.
fn read_registration(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.trim().to_string()).filter(|id| !id.is_empty())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(at(path)(e)),
    }
}

/// Writes `id` to a registration file whole: to a file beside it, then
/// renamed over it.
fn write_registration(path: &Path, id: &str) -> Result<(), String> {
    let mut beside = path.as_os_str().to_owned();
    beside.push(".new");
    std::fs::write(&beside, format!("{id}\n")).map_err(at(path))?;
    std::fs::rename(&beside, path).map_err(at(path))
}

/// The surface a definition names: a manifest's `name`, or the
/// `declare const <name>` of a `.d.ts` file.
fn surface_name(notation: Notation, text: &str) -> Option<String> {
    match notation {
        Notation::Manifest => serde_json::from_str::<Value>(text).ok()?["name"]
            .as_str()
            .map(str::to_string),
        Notation::Dts => {
            let rest = &text[text.find("declare const ")? + "declare const ".len()..];
            Some(rest[..rest.find(':')?].trim().to_string())
        }
    }
}

/// How long the host goes on trying a daemon that does not answer: the
/// pauses of one outage add up to `budget_ms` at the most.
#[derive(Debug)]
struct Patience {
    budget_ms: u64,
    spent_ms: u64,
    next_ms: u64,
}

impl Patience {
    fn new(budget_ms: u64) -> Patience {
        Patience {
            budget_ms,
            spent_ms: 0,
            next_ms: FIRST_PAUSE_MS,
        }
    }

    /// An answer came: the next outage has the whole budget.
    fn answered(&mut self) {
        self.spent_ms = 0;
        self.next_ms = FIRST_PAUSE_MS;
    }

    /// The pause before the next try, or none when the budget is spent.
    fn pause(&mut self) -> Option<u64> {
        let ms = self.next_ms.min(self.budget_ms - self.spent_ms);
        if ms == 0 {
            return None;
        }
        self.spent_ms += ms;
        self.next_ms = (self.next_ms * 2).min(LONGEST_PAUSE_MS);
        Some(ms)
    }
}

fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or("")
}

/// Whether a refusal is of a daemon that is away: no answer at all, or
/// a token it does not take (it restarted, and has a new one).
fn away(status: u16, body: &Value) -> bool {
    matches!(
        (status, code(body)),
        (0, "transport") | (401, "unauthorized")
    )
}

/// The token a token file holds: the file's text, or the `token` of a
/// home's `daemon.json`.
fn read_token(path: &str) -> Result<String, String> {
    let text = read(path)?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(info)) => info
            .get("token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(format!("{path}: holds no token")),
        _ => Ok(text.trim().to_string()),
    }
}

/// Where the serving loop keeps what it learns, and how patient it is.
struct Serving<'a> {
    registration_file: Option<&'a Path>,
    deliveries: Option<&'a Path>,
    retry_for_ms: u64,
}

impl Serving<'_> {
    /// Says that the host registered, keeps the ID, and says when the
    /// ID it meant to go on as was not taken.
    fn registered(
        &self,
        meant: Option<&str>,
        registered: &Registered,
        out: &mut impl Write,
    ) -> Result<(), String> {
        if let Some(meant) = meant.filter(|m| *m != registered.registration) {
            eprintln!(
                "fake_host: the daemon does not know registration {meant}; registered afresh as {}",
                registered.registration
            );
        }
        writeln!(out, "{}", json!(registered)).map_err(|e| e.to_string())?;
        match self.registration_file {
            Some(path) => write_registration(path, &registered.registration),
            None => Ok(()),
        }
    }

    fn saw(&self, deliveries: &[Delivery]) -> Result<(), String> {
        let (Some(path), false) = (self.deliveries, deliveries.is_empty()) else {
            return Ok(());
        };
        let lines: String = deliveries
            .iter()
            .map(|d| format!("{}\n", json!(d)))
            .collect();
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(lines.as_bytes()))
            .map_err(at(path))
    }

    /// Registers `host` (as `meant`, when there is one) and serves until
    /// `stopped` holds of it, as the module's documentation says;
    /// `pause` waits the milliseconds it is given, and `retoken` reads
    /// the token again before a request is tried again. Answers the
    /// host, which has sent the answers it owed.
    async fn serve<T: HostTransport, F: Future<Output = ()>>(
        &self,
        mut host: FakeHost<T>,
        mut meant: Option<String>,
        mut stopped: impl FnMut(&FakeHost<T>) -> bool,
        mut pause: impl FnMut(u64) -> F,
        mut retoken: impl FnMut(&mut T),
        out: &mut impl Write,
    ) -> Result<FakeHost<T>, String> {
        if let Some(id) = &meant {
            host = host.resume(id);
        }
        let mut patience = Patience::new(self.retry_for_ms);
        let mut registering = true;
        while !stopped(&host) {
            let refused = if registering {
                let before = host.registration().map(str::to_string).or(meant.clone());
                match host.register().await {
                    Ok(registered) => {
                        self.registered(before.as_deref(), &registered, out)?;
                        (registering, meant) = (false, None);
                        None
                    }
                    Err(refused) => Some(("register", refused)),
                }
            } else {
                match host.serve_once().await {
                    Served::Refused(404, body) if code(&body) == CODE_UNKNOWN_REGISTRATION => {
                        registering = true;
                        None
                    }
                    Served::Refused(status, body) => Some(("serve", (status, body))),
                    Served::Delivered(deliveries) => {
                        self.saw(&deliveries)?;
                        None
                    }
                    Served::Skipped => None,
                }
            };
            match refused {
                None => patience.answered(),
                Some((what, (status, body))) => {
                    match away(status, &body).then(|| patience.pause()).flatten() {
                        Some(ms) => {
                            pause(ms).await;
                            retoken(host.transport_mut());
                        }
                        None => return Err(format!("{what}: {status} {body}")),
                    }
                }
            }
        }
        if !registering {
            host = host.wait_ms(0);
            if let Served::Delivered(deliveries) = host.serve_once().await {
                self.saw(&deliveries)?;
            }
        }
        Ok(host)
    }
}

async fn run(o: Options) -> Result<(), String> {
    let token = read_token(&o.token_file)?;
    let definition = read(&o.surface)?;
    let notation = if o.surface.ends_with(".d.ts") {
        Notation::Dts
    } else {
        Notation::Manifest
    };
    let name = surface_name(notation, &definition).ok_or("the definition names no surface")?;
    let answers: BTreeMap<String, Value> =
        serde_json::from_str(&read(&o.answers)?).map_err(|e| format!("--answers: {e}"))?;
    let mut host = FakeHost::new(
        HttpTransport::new(&o.url, token),
        &name,
        notation,
        &definition,
    )
    .wait_ms(o.wait_ms);
    for (op, behaviour) in &answers {
        host = host.on(op, Behaviour::from_json(behaviour)?);
    }
    let meant = match (&o.registration, &o.registration_file) {
        (Some(id), _) => Some(id.clone()),
        (None, Some(path)) => read_registration(path)?,
        (None, None) => None,
    };
    let serving = Serving {
        registration_file: o.registration_file.as_deref(),
        deliveries: o.deliveries.as_deref(),
        retry_for_ms: o.retry_for_ms,
    };
    let stop = o.stop_file;
    let host = serving
        .serve(
            host,
            meant,
            |_| stop.exists(),
            |ms| tokio::time::sleep(std::time::Duration::from_millis(ms)),
            |transport: &mut HttpTransport| match read_token(&o.token_file) {
                Ok(token) => transport.set_token(token),
                Err(e) => eprintln!("fake_host: {e}"),
            },
            &mut std::io::stdout(),
        )
        .await?;
    println!(
        "{}",
        json!({"applied": host.applied_by_key(), "deliveries": host.deliveries()})
    );
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let outcome = match parse(std::env::args().skip(1)) {
        Ok(options) => run(options).await,
        Err(e) => Err(e),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fake_host: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "fake_host/tests.rs"]
mod tests;

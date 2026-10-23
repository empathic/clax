//! The sandbox's test route, wasm32 only, with `CLAX_TEST_ROUTES` = `1`
//! (`test-sandbox.mjs` drives it): the conformance suite of
//! [`clax_bot_sandbox::conformance`] and the Worker's own cases, run on the
//! instance's object against [`LoaderSandbox`] one case per request.
//!
//! - `GET /_worker/sandbox`: every case, `[{name, mode, why}]`: `mode`
//!   `run` (the case must pass as on every sandbox), `differs` (a
//!   documented platform difference, [`PLATFORM_DIFFERENCES`]: the run
//!   must end with the error kind named there) or `platform` (needs a
//!   limit only Cloudflare enforces, [`PLATFORM_ONLY`]: not run under
//!   workerd, where it would hold the runtime);
//! - `POST /_worker/sandbox?case=<name>[&port=<port>]`: runs one case,
//!   `{name, ok, problems, outcome}`. `port` is a listener of the
//!   harness's on 127.0.0.1, which the network probe tries to reach; the
//!   harness checks it was never reached.

use clax_bot_sandbox::conformance::{self, OPS};
use clax_bot_sandbox::{
    CODE_CANCELLED, CancelToken, ErrorKind, Host, HostError, Limits, Sandbox, Script, ScriptOutcome,
};
use serde_json::{Value, json};
use worker::{Env, Response, Result};

use super::{LoaderSandbox, PLATFORM_DIFFERENCES, PLATFORM_ONLY};

/// The Worker's own cases, beside the conformance suite's.
const OWN: &[(&str, &str)] = &[
    (
        "worker_without_a_loader_is_unavailable",
        "a sandbox without the LOADER binding ends every run with unavailable",
    ),
    (
        "worker_a_cancel_during_a_host_call_ends_the_run",
        "cancelling while the host answers ends the run as cancelled, with the call's record",
    ),
    (
        "worker_the_watchdog_ends_a_script_busy_between_host_calls",
        "the busy time between host calls counts against the watchdog",
    ),
    (
        "worker_no_network_reaches_a_local_listener",
        "fetch, WebSocket, EventSource, sendBeacon, connect and the Cache API, as globals and from the global's prototypes, all fail",
    ),
];

/// The network probe: each way out a Worker has, as a global and as the
/// runtime's own on the global object's prototypes; what each did.
const NETWORK_PROBE: &str = r#"const url = "http://127.0.0.1:PORT/probe";
const ws = "ws://127.0.0.1:PORT/probe";
const protos = [];
for (let p = Object.getPrototypeOf(globalThis); p !== null; p = Object.getPrototypeOf(p)) protos.push(p);
const pick = (name) => {
  for (const p of protos) {
    const d = Object.getOwnPropertyDescriptor(p, name);
    if (d) return d.get ? d.get.call(globalThis) : d.value;
  }
  return undefined;
};
const tries = [
  ["fetch", () => fetch(url)],
  ["fetch_from_prototype", () => pick("fetch").call(globalThis, url)],
  ["request_fetch", () => pick("fetch").call(globalThis, new Request(url, { method: "POST", body: "x" }))],
  ["websocket", () => new WebSocket(ws)],
  ["websocket_from_prototype", () => new (pick("WebSocket"))(ws)],
  ["websocket_upgrade_fetch", () => pick("fetch").call(globalThis, url, { headers: { upgrade: "websocket" } })],
  ["eventsource", () => new EventSource(url)],
  ["eventsource_from_prototype", () => new (pick("EventSource"))(url)],
  ["send_beacon", () => navigator.sendBeacon(url, "x")],
  ["connect", () => connect("127.0.0.1:PORT")],
  ["connect_from_prototype", () => pick("connect")("127.0.0.1:PORT")],
  ["cache", () => caches.default.match(url)],
  ["cache_from_prototype", () => pick("caches").default.match(url)],
];
const out = {};
for (const [name, attempt] of tries) {
  try {
    const r = await attempt();
    out[name] = r === undefined || r === false ? "nothing" : "made " + Object.prototype.toString.call(r);
  } catch (e) {
    out[name] = "failed " + e.name;
  }
}
return out"#;

fn mode(name: &str) -> (&'static str, &'static str) {
    if let Some((_, why)) = PLATFORM_ONLY.iter().find(|(n, _)| *n == name) {
        return ("platform", why);
    }
    if let Some((_, _, why)) = PLATFORM_DIFFERENCES.iter().find(|(n, _, _)| *n == name) {
        return ("differs", why);
    }
    ("run", "")
}

/// The test route's answer for `path` and `case`, or `None` for a path
/// that is not the route's.
pub async fn route(env: &Env, path: &str, method: &str, query: &str) -> Option<Result<Response>> {
    // Case names are identifiers and the port a number: no decoding.
    let param = |key: &str| {
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
    };
    let case = param("case");
    let port = param("port").and_then(|p| p.parse::<u16>().ok());
    if path != "/_worker/sandbox" {
        return None;
    }
    if method == "GET" {
        let mut all: Vec<Value> = conformance::cases()
            .iter()
            .map(|c| {
                let (mode, why) = mode(c.name);
                json!({"name": c.name, "mode": mode, "why": why})
            })
            .collect();
        all.extend(
            OWN.iter()
                .map(|(name, why)| json!({"name": name, "mode": "run", "why": why})),
        );
        return Some(Response::from_json(&all));
    }
    let Some(name) = case else {
        return Some(Response::error("case is required", 400));
    };
    let sandbox = match LoaderSandbox::from_env(env, OPS.iter().copied()) {
        Ok(s) => s,
        Err(e) => return Some(Response::error(e, 500)),
    };
    let result = match conformance::cases().into_iter().find(|c| c.name == name) {
        Some(case) => match PLATFORM_DIFFERENCES.iter().find(|(n, _, _)| *n == name) {
            Some((_, kind, _)) => {
                let host = conformance::ScriptedHost::new(case.calls.clone());
                let outcome = sandbox
                    .run(
                        &Script::new(case.name, case.code),
                        &host,
                        &case.limits,
                        &CancelToken::new(),
                    )
                    .await;
                let got = outcome.error.as_ref().map(|e| e.kind);
                if got == Some(*kind) {
                    Ok(outcome)
                } else {
                    Err((
                        format!("expected the documented {kind:?}, got {:?}", outcome.error),
                        Some(outcome),
                    ))
                }
            }
            None => conformance::check(&sandbox, &case)
                .await
                .map_err(|e| (e, None)),
        },
        None => own(&sandbox, name, port).await,
    };
    Some(Response::from_json(&match result {
        Ok(outcome) => json!({"name": name, "ok": true, "problems": "", "outcome": outcome}),
        Err((problems, outcome)) => {
            json!({"name": name, "ok": false, "problems": problems, "outcome": outcome})
        }
    }))
}

type Checked = std::result::Result<ScriptOutcome, (String, Option<ScriptOutcome>)>;

fn expect(outcome: ScriptOutcome, ok: bool, what: &str) -> Checked {
    if ok {
        Ok(outcome)
    } else {
        Err((what.to_string(), Some(outcome)))
    }
}

/// Cancels the run's token from inside the host's first call.
struct CancellingHost(CancelToken);

#[async_trait::async_trait(?Send)]
impl Host for CancellingHost {
    async fn call(&self, _op: &str, _args: Value) -> std::result::Result<Value, HostError> {
        self.0.cancel();
        std::future::pending::<()>().await;
        Ok(Value::Null)
    }
}

/// Answers every call with its arguments.
struct Echo;

#[async_trait::async_trait(?Send)]
impl Host for Echo {
    async fn call(&self, _op: &str, args: Value) -> std::result::Result<Value, HostError> {
        Ok(args)
    }
}

async fn own(sandbox: &LoaderSandbox, name: &str, port: Option<u16>) -> Checked {
    let limits = Limits::default();
    match name {
        "worker_without_a_loader_is_unavailable" => {
            let bare =
                LoaderSandbox::with_binding(None, OPS.iter().copied()).map_err(|e| (e, None))?;
            let outcome = bare
                .run(
                    &Script::new(name, "return 1"),
                    &Echo,
                    &limits,
                    &CancelToken::new(),
                )
                .await;
            let kind = outcome.error.as_ref().map(|e| e.kind);
            expect(
                outcome,
                kind == Some(ErrorKind::Unavailable),
                "expected unavailable",
            )
        }
        "worker_a_cancel_during_a_host_call_ends_the_run" => {
            let cancel = CancelToken::new();
            let outcome = sandbox
                .run(
                    &Script::new(
                        name,
                        r#"console.log("before"); await clax.echo(1); return 2"#,
                    ),
                    &CancellingHost(cancel.clone()),
                    &limits,
                    &cancel,
                )
                .await;
            let ok = outcome.error.as_ref().map(|e| e.kind) == Some(ErrorKind::Cancelled)
                && outcome.logs == ["before"]
                && outcome.calls.len() == 1
                && outcome.calls[0].code.as_deref() == Some(CODE_CANCELLED);
            expect(
                outcome,
                ok,
                "expected cancelled, the log line and the call's record",
            )
        }
        "worker_the_watchdog_ends_a_script_busy_between_host_calls" => {
            // Each turn is busy past its share of the watchdog's 500 ms
            // floor (cpu_ms 50 × 4 is under it) before it calls the host.
            let limits = Limits {
                cpu_ms: 50,
                ..Limits::default()
            };
            let code = "for (let i = 0; i < 100; i++) { let n = 0; while (n < 3e7) n++; await clax.echo(i) } return 'escaped'";
            let outcome = sandbox
                .run(
                    &Script::new(name, code),
                    &Echo,
                    &limits,
                    &CancelToken::new(),
                )
                .await;
            let kind = outcome.error.as_ref().map(|e| e.kind);
            let busy = outcome.busy_us;
            expect(
                outcome,
                kind == Some(ErrorKind::CpuLimit) && busy >= 500_000,
                "expected the watchdog's cpu_limit after at least 500 ms busy",
            )
        }
        "worker_no_network_reaches_a_local_listener" => {
            let Some(port) = port else {
                return Err(("the probe needs the harness's port".into(), None));
            };
            let code = NETWORK_PROBE.replace("PORT", &port.to_string());
            let outcome = sandbox
                .run(
                    &Script::new(name, code),
                    &Echo,
                    &limits,
                    &CancelToken::new(),
                )
                .await;
            let made: Vec<String> = match &outcome.value {
                Some(Value::Object(tries)) => tries
                    .iter()
                    .filter(|(_, v)| {
                        !v.as_str()
                            .is_some_and(|v| v.starts_with("failed") || v == "nothing")
                    })
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect(),
                _ => vec!["no answer".into()],
            };
            let ok = outcome.error.is_none() && made.is_empty();
            expect(outcome, ok, &format!("a way out did not fail: {made:?}"))
        }
        _ => Err((format!("no case {name:?}"), None)),
    }
}

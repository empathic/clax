//! Daytona's REST API (`https://www.daytona.io/docs/openapi.json` and
//! `toolbox-openapi.json`) behind [`ContainerRuntime`].
//!
//! `process/execute` splits `command` into arguments rather than giving it
//! to a shell, and its `timeout` defaults to 10 seconds. So every call goes
//! through [`shell_command`] and names a timeout.
//!
//! An answer of 429 did nothing, so every call may repeat on it. A 5xx or a
//! lost connection may have done something, so only the calls that mean the
//! same when repeated go again on those ([`Repeat`]). A create whose answer
//! was lost is settled by reading its name instead.
//!
//! What a sandbox was asked to enforce and what its runner enforces can
//! differ, and `networkPolicyError` says why. So a sandbox is only handed out
//! once it reports the allow-list it was made with and no such error
//! ([`holds_egress`]); otherwise it is deleted.
//!
//! A sandbox's CPU, memory and disk are its snapshot's. `CreateSandbox` lists
//! them too, but Daytona's SDKs take them only for a sandbox made from an image,
//! so [`create_body`] sends none.

use crate::repo::Token;
use crate::runtime::{
    Container, ContainerRuntime, ContainerSpec, EXIT_TIMED_OUT, Exec, ExecOutput, Found, Gone,
    MAX_EXEC_OUTPUT, bounded, control_client, is_gone,
};
use anyhow::{Context, Result, bail};
use base64::Engine;
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

pub const DEFAULT_API_URL: &str = "https://app.daytona.io/api";

/// Extra time an HTTP call gets over the command's own timeout.
const HTTP_GRACE: Duration = Duration::from_secs(30);

/// The most a `process/execute` answer may hold: a command's kept output,
/// JSON-escaped.
const MAX_EXECUTE_ANSWER: usize = 2 * MAX_EXEC_OUTPUT;

/// The most of any other answer that is read.
const MAX_ANSWER: usize = 8 * 1024 * 1024;

/// The most of a failure's body that is read.
const MAX_ERROR_BODY: usize = 16 * 1024;

/// The longest one wait between attempts, whatever a `Retry-After` says.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A page of `GET /sandbox`, its documented maximum.
const PAGE: &str = "200";

/// Minutes a stopped sandbox stays before Daytona deletes it, so one a
/// crashed daemon left does not stay for good. Long past any run.
const AUTO_DELETE_MINUTES: u32 = 24 * 60;

/// `command` as one `process/execute` command line: base64, so that no
/// quoting of it survives to be got wrong. It is given to `sh` as an
/// argument and not on stdin, which a command in it would otherwise read.
pub fn shell_command(command: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(command);
    format!("sh -c 'exec sh -c \"$(echo {b64} | base64 -d)\" </dev/null'")
}

/// When a call goes again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Repeat {
    /// On a 429 only: repeating it after anything else could do it twice.
    RateLimited,
    /// On a 429, a 5xx or a lost connection.
    Idempotent,
}

impl Repeat {
    fn covers(self, status: StatusCode) -> bool {
        status == StatusCode::TOO_MANY_REQUESTS
            || (self == Repeat::Idempotent && status.is_server_error())
    }
}

/// How to reach a Daytona API.
#[derive(Clone, Debug)]
pub struct Daytona {
    api_url: String,
    api_key: Token,
    organization: Option<String>,
    http: Client,
    /// How long a sandbox may take to start.
    pub start_timeout: Duration,
    pub poll: Duration,
    /// How many times a call goes again.
    pub retries: u32,
    /// The wait before the first repeat, doubled for each after it.
    pub backoff: Duration,
}

impl Daytona {
    pub fn new(api_url: &str, api_key: &str, organization: Option<String>) -> Daytona {
        Daytona {
            api_url: api_url.trim_end_matches('/').to_string(),
            api_key: Token::new(api_key),
            organization,
            http: control_client(),
            start_timeout: Duration::from_secs(180),
            poll: Duration::from_millis(500),
            retries: 5,
            backoff: Duration::from_millis(500),
        }
    }

    /// From `DAYTONA_API_KEY`, `DAYTONA_API_URL` and
    /// `DAYTONA_ORGANIZATION_ID`.
    pub fn from_env() -> Result<Daytona> {
        let key = std::env::var("DAYTONA_API_KEY").context("DAYTONA_API_KEY is not set")?;
        let url = std::env::var("DAYTONA_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.into());
        Ok(Daytona::new(
            &url,
            &key,
            std::env::var("DAYTONA_ORGANIZATION_ID").ok(),
        ))
    }

    fn auth(&self, r: RequestBuilder) -> RequestBuilder {
        let r = r.bearer_auth(self.api_key.expose());
        match &self.organization {
            Some(o) => r.header("X-Daytona-Organization-ID", o),
            None => r,
        }
    }

    /// Sends what `request` builds, authenticated, as often as `repeat`
    /// allows. Any answer not repeated comes back, a failure too.
    async fn send(
        &self,
        what: &str,
        repeat: Repeat,
        request: impl Fn() -> RequestBuilder,
    ) -> Result<Response> {
        let mut wait = self.backoff;
        let mut attempt = 0;
        loop {
            let last = attempt >= self.retries;
            attempt += 1;
            match self.auth(request()).send().await {
                Ok(r) if !last && repeat.covers(r.status()) => {
                    tracing::debug!(status = %r.status(), what, "Daytona: going again");
                    let told = retry_after(&r).unwrap_or(wait);
                    tokio::time::sleep(told.min(MAX_BACKOFF)).await;
                }
                Ok(r) => return Ok(r),
                Err(e) if !last && repeat == Repeat::Idempotent => {
                    tracing::debug!(error = %e, what, "Daytona: going again");
                    tokio::time::sleep(wait.min(MAX_BACKOFF)).await;
                }
                Err(e) => return Err(e).with_context(|| format!("{what}: no answer from Daytona")),
            }
            wait *= 2;
        }
    }

    /// Sandbox `id_or_name`, [`Gone`] on a 404.
    async fn state(&self, id_or_name: &str) -> Result<SandboxState> {
        let what = "reading a sandbox";
        let url = format!("{}/sandbox/{id_or_name}", self.api_url);
        let r = self
            .send(what, Repeat::Idempotent, || self.http.get(&url))
            .await?;
        if r.status() == StatusCode::NOT_FOUND {
            return Err(Gone(format!("no sandbox {id_or_name}")).into());
        }
        answer(what, r).await
    }

    /// Posts `action` for a sandbox: `start` for a stopped, archived or
    /// paused one, `recover` for one in a recoverable error.
    async fn wake(&self, id: &str, action: &str) -> Result<()> {
        let what = "starting a sandbox";
        let url = format!("{}/sandbox/{id}/{action}", self.api_url);
        let r = self
            .send(what, Repeat::Idempotent, || self.http.post(&url))
            .await?;
        let status = r.status();
        if !status.is_success() {
            bail!("{}", failure(what, status, r).await);
        }
        Ok(())
    }

    /// Follows sandbox `s` until it is `started`. With `wake` it also starts
    /// one at rest; otherwise that state is only waited out. A sandbox that
    /// will never start is [`Gone`].
    async fn until_started(&self, mut s: SandboxState, wake: bool) -> Result<SandboxState> {
        let deadline = Instant::now() + self.start_timeout;
        let mut woken = false;
        loop {
            match s.state.as_deref() {
                Some("started") => return Ok(s),
                Some("error") if wake && s.recoverable && !woken => {
                    self.wake(&s.id, "recover").await?;
                    woken = true;
                }
                Some("error") if wake && s.recoverable => {
                    let why = s.error_reason.as_deref().unwrap_or("no reason given");
                    bail!("sandbox {} did not recover: {why}", s.id);
                }
                Some(bad @ ("error" | "build_failed" | "destroyed" | "destroying")) => {
                    let why = s.error_reason.as_deref().unwrap_or("no reason given");
                    return Err(Gone(format!("sandbox {} is {bad}: {why}", s.id)).into());
                }
                Some("stopped" | "archived" | "paused") if wake && !woken => {
                    self.wake(&s.id, "start").await?;
                    woken = true;
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                bail!(
                    "sandbox {} is still {} after {}s",
                    s.id,
                    s.state.as_deref().unwrap_or("in an unknown state"),
                    self.start_timeout.as_secs()
                );
            }
            tokio::time::sleep(self.poll).await;
            s = self.state(&s.id).await?;
        }
    }

    /// Deletes sandbox `id`, which `why` says may not be used: one that
    /// never started still counts against the account, and one running open
    /// must not stay.
    async fn discard(&self, id: &str, why: anyhow::Error) -> anyhow::Error {
        if let Err(d) = self.delete(id).await {
            tracing::warn!(error = %d, sandbox = id, "deleting a failed sandbox failed");
        }
        why
    }

    fn handle(&self, s: SandboxState) -> DaytonaSandbox {
        DaytonaSandbox {
            daytona: self.clone(),
            execute_url: format!(
                "{}/{}/process/execute",
                s.toolbox_proxy_url.trim_end_matches('/'),
                s.id
            ),
            id: s.id,
        }
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let what = "deleting a sandbox";
        let url = format!("{}/sandbox/{id}", self.api_url);
        let r = self
            .send(what, Repeat::Idempotent, || self.http.delete(&url))
            .await?;
        let status = r.status();
        // Gone is what was asked for, as after a repeat of a lost answer.
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        bail!("{}", failure(what, status, r).await);
    }
}

/// Fails unless `s` reports it enforces just what `spec` asks. A runtime
/// that cannot say so is [`Gone`]: such a sandbox is not to be used.
fn holds_egress(s: &SandboxState, spec: &ContainerSpec) -> Result<()> {
    if let Some(e) = s.network_policy_error.as_deref().filter(|e| !e.is_empty()) {
        return Err(Gone(format!("sandbox {} has no network policy: {e}", s.id)).into());
    }
    let told = s.domain_allow_list.as_deref().unwrap_or_default();
    let told: BTreeSet<&str> = told
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .collect();
    let asked: BTreeSet<&str> = spec.domain_allow_list.iter().map(String::as_str).collect();
    if told != asked {
        let id = &s.id;
        return Err(Gone(format!(
            "sandbox {id} reports allow-list {told:?}, not {asked:?}"
        ))
        .into());
    }
    Ok(())
}

/// The wait a `Retry-After` of seconds names.
fn retry_after(r: &Response) -> Option<Duration> {
    let v = r.headers().get(reqwest::header::RETRY_AFTER)?;
    let secs: u64 = v.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

/// The fields of a `Sandbox` read here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandboxState {
    id: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error_reason: Option<String>,
    #[serde(default)]
    recoverable: bool,
    #[serde(default)]
    domain_allow_list: Option<String>,
    #[serde(default)]
    network_policy_error: Option<String>,
    toolbox_proxy_url: String,
}

/// The fields of a `SandboxListItem` read here.
#[derive(Debug, Deserialize)]
struct ListItem {
    id: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

/// A `ListSandboxesResponse`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListPage {
    items: Vec<ListItem>,
    #[serde(default)]
    next_cursor: Option<String>,
}

/// `exitCode` is optional in the API; its absence is read as `0`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteResponse {
    #[serde(default)]
    exit_code: Option<i32>,
    result: String,
}

/// What failed, for the log. Unclipped past the read's own bound, so that
/// what redacts it sees whole any token an API echoed.
async fn failure(what: &str, status: StatusCode, r: Response) -> String {
    let body = bounded(r, MAX_ERROR_BODY).await.ok().flatten();
    let body = String::from_utf8_lossy(body.as_deref().unwrap_or_default()).into_owned();
    let told = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .unwrap_or(body);
    describe(what, status, &told)
}

fn describe(what: &str, status: StatusCode, told: &str) -> String {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            format!("{what}: Daytona refused the API key or organization ({status}): {told}")
        }
        StatusCode::NOT_FOUND => format!("{what}: Daytona has no such sandbox ({status}): {told}"),
        StatusCode::TOO_MANY_REQUESTS => {
            format!("{what}: Daytona's rate limit held through every retry ({status}): {told}")
        }
        _ => format!("{what}: Daytona answered {status}: {told}"),
    }
}

async fn answer<T: serde::de::DeserializeOwned>(what: &str, r: Response) -> Result<T> {
    let status = r.status();
    if !status.is_success() {
        bail!("{}", failure(what, status, r).await);
    }
    let body = bounded(r, MAX_ANSWER)
        .await?
        .with_context(|| format!("{what}: Daytona's answer is over the limit"))?;
    serde_json::from_slice(&body).with_context(|| format!("{what}: Daytona's answer"))
}

/// `text` cut to [`MAX_EXEC_OUTPUT`] on a character boundary.
fn kept(mut text: String) -> String {
    if text.len() > MAX_EXEC_OUTPUT {
        let mut end = MAX_EXEC_OUTPUT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// The `CreateSandbox` body for `spec`.
fn create_body(spec: &ContainerSpec) -> Value {
    let mut b = Map::new();
    b.insert("autoDeleteInterval".into(), json!(AUTO_DELETE_MINUTES));
    if let Some(n) = &spec.name {
        b.insert("name".into(), json!(n));
    }
    if let Some(s) = &spec.image {
        b.insert("snapshot".into(), json!(s));
    }
    if !spec.labels.is_empty() {
        b.insert("labels".into(), json!(spec.labels));
    }
    if let Some(m) = spec.auto_stop_minutes {
        b.insert("autoStopInterval".into(), json!(m));
    }
    if !spec.domain_allow_list.is_empty() {
        b.insert(
            "domainAllowList".into(),
            json!(spec.domain_allow_list.join(",")),
        );
    }
    Value::Object(b)
}

#[async_trait::async_trait]
impl ContainerRuntime for Daytona {
    fn limits_domains(&self) -> bool {
        true
    }

    async fn create(&self, spec: &ContainerSpec) -> Result<Box<dyn Container>> {
        let what = "making a sandbox";
        let url = format!("{}/sandbox", self.api_url);
        let body = create_body(spec);
        let asked = self
            .send(what, Repeat::RateLimited, || {
                self.http.post(&url).json(&body)
            })
            .await;
        let made: Result<SandboxState> = match asked {
            Ok(r) => answer(what, r).await,
            Err(e) => Err(e),
        };
        let s = match (made, &spec.name) {
            (Ok(s), _) => s,
            // It may be there all the same.
            (Err(e), Some(name)) => self.state(name).await.map_err(|_| e)?,
            (Err(e), None) => return Err(e),
        };
        let id = s.id.clone();
        let started = self.until_started(s, false).await;
        match started.and_then(|s| holds_egress(&s, spec).map(|()| s)) {
            Ok(s) => Ok(Box::new(self.handle(s))),
            Err(e) => Err(self.discard(&id, e).await),
        }
    }

    async fn attach(&self, id: &str, spec: &ContainerSpec) -> Result<Box<dyn Container>> {
        let s = self.state(id).await?;
        let started = match self.until_started(s, true).await {
            Ok(s) => s,
            Err(e) if is_gone(&e) => return Err(self.discard(id, e).await),
            // It may start yet: a run still needs what is in it.
            Err(e) => return Err(e),
        };
        match holds_egress(&started, spec) {
            Ok(()) => Ok(Box::new(self.handle(started))),
            Err(e) => Err(self.discard(id, e).await),
        }
    }

    async fn find(&self, labels: &BTreeMap<String, String>) -> Result<Vec<Found>> {
        let what = "listing sandboxes";
        let url = format!("{}/sandbox", self.api_url);
        let filter = serde_json::to_string(labels)?;
        let mut found = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let r = self
                .send(what, Repeat::Idempotent, || {
                    let mut q = vec![("labels", filter.as_str()), ("limit", PAGE)];
                    if let Some(c) = &cursor {
                        q.push(("cursor", c));
                    }
                    self.http.get(&url).query(&q)
                })
                .await?;
            let page: ListPage = answer(what, r).await?;
            found.extend(
                page.items
                    .into_iter()
                    .filter(|i| !matches!(i.state.as_deref(), Some("destroyed" | "destroying")))
                    .map(|i| Found {
                        id: i.id,
                        labels: i.labels,
                    })
                    // A caller may delete what this answers, so the filter
                    // is not left to the server alone.
                    .filter(|f| f.labels_hold(labels)),
            );
            cursor = page.next_cursor.filter(|c| !c.is_empty());
            if cursor.is_none() {
                return Ok(found);
            }
        }
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        self.delete(id).await
    }
}

/// One sandbox of a [`Daytona`].
#[derive(Debug)]
pub struct DaytonaSandbox {
    daytona: Daytona,
    id: String,
    execute_url: String,
}

#[async_trait::async_trait]
impl Container for DaytonaSandbox {
    fn id(&self) -> &str {
        &self.id
    }

    async fn exec(&self, exec: &Exec) -> Result<ExecOutput> {
        let what = "running a command";
        let mut body = json!({
            "command": shell_command(&exec.command),
            "timeout": exec.timeout.as_secs().max(1),
        });
        if let Some(cwd) = &exec.cwd {
            body["cwd"] = json!(cwd);
        }
        if !exec.envs.is_empty() {
            body["envs"] = json!(exec.envs);
        }
        let d = &self.daytona;
        let r = d
            .send(what, Repeat::RateLimited, || {
                d.http
                    .post(&self.execute_url)
                    .timeout(exec.timeout + HTTP_GRACE)
                    .json(&body)
            })
            .await?;
        let status = r.status();
        // The toolbox documents 408 for this call.
        if status == StatusCode::REQUEST_TIMEOUT {
            return Ok(ExecOutput {
                exit_code: EXIT_TIMED_OUT,
                output: "the command did not stop in time".to_string(),
            });
        }
        if !status.is_success() {
            bail!("{}", failure(what, status, r).await);
        }
        let Some(raw) = bounded(r, MAX_EXECUTE_ANSWER).await? else {
            return Ok(ExecOutput {
                exit_code: -1,
                output: "the command wrote more output than is kept".to_string(),
            });
        };
        let e: ExecuteResponse =
            serde_json::from_slice(&raw).with_context(|| format!("{what}: Daytona's answer"))?;
        Ok(ExecOutput {
            exit_code: e.exit_code.unwrap_or(0),
            output: kept(e.result),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_survives_quoting_as_base64() {
        let c = shell_command("echo 'a b' | tr a-z A-Z");
        let b64 = c
            .strip_prefix("sh -c 'exec sh -c \"$(echo ")
            .and_then(|r| r.strip_suffix(" | base64 -d)\" </dev/null'"))
            .unwrap();
        let back = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(back, b"echo 'a b' | tr a-z A-Z");
    }

    #[test]
    fn a_create_body_sends_only_what_the_spec_names() {
        assert_eq!(
            create_body(&ContainerSpec::default()),
            json!({"autoDeleteInterval": AUTO_DELETE_MINUTES})
        );
        let spec = ContainerSpec {
            image: Some("s".into()),
            name: Some("clax-r1".into()),
            auto_stop_minutes: Some(15),
            domain_allow_list: vec!["a.dev".into(), "b.dev".into()],
            ..ContainerSpec::default()
        };
        assert_eq!(
            create_body(&spec),
            json!({
                "autoDeleteInterval": AUTO_DELETE_MINUTES,
                "name": "clax-r1",
                "snapshot": "s",
                "autoStopInterval": 15,
                "domainAllowList": "a.dev,b.dev",
            })
        );
    }

    #[test]
    fn only_an_idempotent_call_repeats_on_a_server_error() {
        let busy = StatusCode::TOO_MANY_REQUESTS;
        let down = StatusCode::BAD_GATEWAY;
        assert!(Repeat::RateLimited.covers(busy));
        assert!(!Repeat::RateLimited.covers(down));
        assert!(Repeat::Idempotent.covers(down));
        assert!(!Repeat::Idempotent.covers(StatusCode::NOT_FOUND));
    }

    #[test]
    fn a_refused_key_is_named_as_that() {
        let text = describe("listing", StatusCode::UNAUTHORIZED, "Invalid API key");
        assert!(text.contains("refused the API key"), "{text}");
        assert!(text.contains("Invalid API key"), "{text}");
    }

    #[test]
    fn long_output_is_cut_on_a_character_boundary() {
        let text = kept("é".repeat(MAX_EXEC_OUTPUT));
        assert!(text.len() <= MAX_EXEC_OUTPUT);
        assert_eq!(kept("short".into()), "short");
    }

    #[test]
    fn an_allow_list_is_compared_as_a_set() {
        let s = |list: Option<&str>, error: Option<&str>| SandboxState {
            id: "s".into(),
            state: None,
            error_reason: None,
            recoverable: false,
            domain_allow_list: list.map(str::to_string),
            network_policy_error: error.map(str::to_string),
            toolbox_proxy_url: String::new(),
        };
        let spec = ContainerSpec {
            domain_allow_list: vec!["a.dev".into(), "b.dev".into()],
            ..ContainerSpec::default()
        };
        assert!(holds_egress(&s(Some("b.dev, a.dev"), None), &spec).is_ok());
        assert!(holds_egress(&s(Some("a.dev"), None), &spec).is_err());
        assert!(holds_egress(&s(None, None), &spec).is_err());
        assert!(holds_egress(&s(Some("a.dev,b.dev"), Some("no")), &spec).is_err());
        assert!(holds_egress(&s(None, None), &ContainerSpec::default()).is_ok());
    }
}

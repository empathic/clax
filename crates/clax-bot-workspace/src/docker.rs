//! The `docker` command behind [`ContainerRuntime`]: a machine's own
//! containers, for development and for hosts that run their own.
//!
//! Docker has no way to limit a container to named hosts, so a spec with an
//! allow-list is refused. With no allow-list a container has whatever network
//! `run_args` gives it, by default open egress.
//!
//! A container has no capability and a bounded share of the machine unless
//! `run_args` says otherwise, so an image must come with its tools built in:
//! `apt` and anything else that changes owners fails inside. `--cap-add` in
//! `run_args` is the way out for an image that needs more. It idles out by its own watch ([`idle_watch`]):
//! every command marks it active, and with no command for the spec's minutes
//! its first process ends. That watch runs inside, so what runs there can
//! keep it alive for as long as it likes, and no longer than the run's
//! `clax_bot::Workspace::reap`.

use crate::runtime::{
    Container, ContainerRuntime, ContainerSpec, EXIT_TIMED_OUT, Exec, ExecOutput, Found, Gone,
    MAX_EXEC_OUTPUT,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt};

const DEFAULT_IMAGE: &str = "ubuntu:24.04";

/// Marks containers as this crate's, so `attach` touches no other.
const LABEL: &str = "clax.runtime=docker";

/// What a container gets of the machine, before `run_args`.
const LIMITS: [&str; 8] = [
    "--pids-limit",
    "4096",
    "--memory",
    "8g",
    "--cap-drop",
    "ALL",
    "--security-opt",
    "no-new-privileges",
];

/// Touched by every command, for [`idle_watch`].
const ACTIVE: &str = "/tmp/.clax-active";
const IDLE_POLL_SECS: u64 = 30;

/// Seconds between `timeout`'s SIGTERM and its SIGKILL.
const KILL_AFTER_SECS: u64 = 5;

/// Extra time `docker exec` gets over the `timeout` inside the container,
/// which is the container's own binary and may not stop anything.
const EXEC_GRACE: Duration = Duration::from_secs(KILL_AFTER_SECS + 5);

/// How long a `docker` command of this crate's own may take; a first `run`
/// pulls its image.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(600);

/// The machine's Docker.
#[derive(Clone, Debug, Default)]
pub struct Docker {
    /// Extra `docker run` arguments.
    pub run_args: Vec<String>,
}

/// What one `docker` command did.
struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Ran {
    /// Both streams, at most [`MAX_EXEC_OUTPUT`] of them together.
    fn text(&self) -> String {
        let mut text = format!("{}{}", self.stdout, self.stderr);
        if text.len() > MAX_EXEC_OUTPUT {
            let mut end = MAX_EXEC_OUTPUT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        text
    }

    fn no_such_container(&self) -> bool {
        self.stderr.contains("No such container")
    }
}

/// Reads `from` to its end, keeping the first `limit` bytes.
async fn read_capped(from: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut from = from;
    let mut kept = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = from.read(&mut chunk).await.context("reading docker")?;
        if n == 0 {
            return Ok(kept);
        }
        let room = limit - kept.len();
        kept.extend_from_slice(&chunk[..n.min(room)]);
    }
}

/// What the `docker` CLI keeps of the daemon's environment: how it finds its
/// engine and its configuration, and nothing else, since the daemon's also
/// holds an App's private key and tokens.
const DOCKER_ENV: [&str; 16] = [
    "PATH",
    "HOME",
    "TMPDIR",
    "USER",
    "XDG_RUNTIME_DIR",
    "SSH_AUTH_SOCK",
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_CONFIG",
    "DOCKER_CERT_PATH",
    "DOCKER_TLS_VERIFY",
    "DOCKER_API_VERSION",
    "http_proxy",
    "https_proxy",
    "HTTPS_PROXY",
    "NO_PROXY",
];

/// What of `get`, an environment, the `docker` CLI keeps.
fn kept_env(get: impl Fn(&str) -> Option<OsString>) -> Vec<(&'static str, OsString)> {
    DOCKER_ENV
        .iter()
        .filter_map(|k| Some((*k, get(k)?)))
        .collect()
}

/// Runs `docker args` with `envs` on its own environment, where a value
/// stays out of the host's process list.
async fn run(args: &[String], envs: &BTreeMap<String, String>) -> Result<Ran> {
    let mut child = tokio::process::Command::new("docker")
        .env_clear()
        .envs(kept_env(|k| std::env::var_os(k)))
        .args(args)
        .envs(envs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("starting docker")?;
    let stdout = child.stdout.take().context("docker's stdout")?;
    let stderr = child.stderr.take().context("docker's stderr")?;
    let (out, err, status) = tokio::join!(
        read_capped(stdout, MAX_EXEC_OUTPUT),
        read_capped(stderr, MAX_EXEC_OUTPUT),
        child.wait()
    );
    Ok(Ran {
        code: status.context("waiting for docker")?.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out?).into_owned(),
        stderr: String::from_utf8_lossy(&err?).into_owned(),
    })
}

/// A `docker` command of this crate's own, bounded in time.
async fn docker(args: &[String]) -> Result<Ran> {
    tokio::time::timeout(CONTROL_TIMEOUT, run(args, &BTreeMap::new()))
        .await
        .with_context(|| format!("docker {} did not finish", args[0]))?
}

/// Whether a Docker endpoint is on this machine: a socket, or TCP to
/// loopback.
fn local_endpoint(endpoint: &str) -> bool {
    let Some((scheme, rest)) = endpoint.split_once("://") else {
        return false;
    };
    match scheme {
        "unix" | "npipe" => true,
        "tcp" | "http" | "https" => {
            let host = rest.split('/').next().unwrap_or_default();
            let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
            matches!(host, "localhost" | "127.0.0.1" | "[::1]")
        }
        _ => false,
    }
}

fn strings<const N: usize>(args: [&str; N]) -> Vec<String> {
    args.map(String::from).to_vec()
}

/// The shell text of a container's first process: it ends once [`ACTIVE`]
/// has not been touched for `seconds`, looking every `poll`.
pub fn idle_watch(seconds: u64, poll: u64) -> String {
    format!(
        "touch {ACTIVE}; while :; do \
         last=$(stat -c %Y {ACTIVE} 2>/dev/null || date +%s); \
         [ $(( $(date +%s) - last )) -lt {seconds} ] || exit 0; sleep {poll}; done"
    )
}

/// A name for a container whose spec gives none.
fn fresh_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("clax-{nanos}-{}", std::process::id())
}

/// Refuses what Docker cannot hold a container to.
fn check_spec(spec: &ContainerSpec) -> Result<()> {
    if !spec.domain_allow_list.is_empty() {
        bail!(
            "this runtime cannot limit a container to named hosts: \
             clear domain_allow_list, or use a runtime that can"
        );
    }
    Ok(())
}

impl Docker {
    /// The `docker run` arguments for `spec`, as container `name`.
    fn create_args(&self, spec: &ContainerSpec, name: &str) -> Result<Vec<String>> {
        check_spec(spec)?;
        let mut args = strings([
            "run", "--detach", "--init", "--label", LABEL, "--name", name,
        ]);
        args.extend(LIMITS.map(String::from));
        for (k, v) in &spec.labels {
            args.extend(["--label".to_string(), format!("{k}={v}")]);
        }
        args.extend(self.run_args.iter().cloned());
        args.push(spec.image.clone().unwrap_or_else(|| DEFAULT_IMAGE.into()));
        let seconds = u64::from(spec.auto_stop_minutes.unwrap_or(60)) * 60;
        args.extend(strings(["sh", "-c", &idle_watch(seconds, IDLE_POLL_SECS)]));
        Ok(args)
    }
}

/// The `docker ps` arguments listing this crate's containers among `filters`.
fn list_args(filters: &[String]) -> Vec<String> {
    let mut args = strings(["ps", "--all", "--quiet", "--no-trunc"]);
    args.extend(["--filter".to_string(), format!("label={LABEL}")]);
    for f in filters {
        args.extend(["--filter".to_string(), f.clone()]);
    }
    args
}

/// Reads `docker inspect`'s `<id> <labels as JSON>` lines.
fn parse_found(text: &str) -> Result<Vec<Found>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let (id, labels) = line.split_once(' ').context("a line of docker inspect")?;
            Ok(Found {
                id: id.to_string(),
                labels: serde_json::from_str(labels).context("a container's labels")?,
            })
        })
        .collect()
}

#[async_trait::async_trait]
impl ContainerRuntime for Docker {
    /// Whether the engine this `docker` talks to is elsewhere: another
    /// machine, by `DOCKER_HOST` or its context. One it cannot name counts
    /// as elsewhere.
    fn third_party(&self) -> bool {
        let asked = std::process::Command::new("docker")
            .args([
                "context",
                "inspect",
                "--format",
                "{{.Endpoints.docker.Host}}",
            ])
            .env_clear()
            .envs(kept_env(|k| std::env::var_os(k)))
            .output();
        match asked {
            Ok(out) if out.status.success() => {
                !local_endpoint(String::from_utf8_lossy(&out.stdout).trim())
            }
            _ => true,
        }
    }

    fn limits_domains(&self) -> bool {
        false
    }

    async fn create(&self, spec: &ContainerSpec) -> Result<Box<dyn Container>> {
        let name = spec.name.clone().unwrap_or_else(fresh_name);
        let r = docker(&self.create_args(spec, &name)?).await?;
        if r.code != 0 {
            // A container made and failed at its start would stay behind.
            // A name taken is another's, or this one's from a lost answer.
            if !r.stderr.contains("is already in use") {
                self.destroy(&name).await.ok();
            }
            bail!("docker run failed: {}", r.text().trim());
        }
        // stdout alone: a first use of an image reports its pull on stderr.
        let id = r.stdout.lines().last().unwrap_or_default().trim();
        Ok(Box::new(DockerContainer { id: id.to_string() }))
    }

    async fn attach(&self, id: &str, spec: &ContainerSpec) -> Result<Box<dyn Container>> {
        check_spec(spec)?;
        let r = docker(&list_args(&[format!("id={id}")])).await?;
        if r.code != 0 {
            bail!("docker ps failed: {}", r.text().trim());
        }
        let found = r.stdout.trim();
        if found.is_empty() {
            return Err(Gone(format!("no container {id}")).into());
        }
        let id = found.to_string();
        let r = docker(&strings(["start", &id])).await?;
        if r.code != 0 {
            bail!("docker start failed: {}", r.text().trim());
        }
        Ok(Box::new(DockerContainer { id }))
    }

    async fn find(&self, labels: &BTreeMap<String, String>) -> Result<Vec<Found>> {
        let filters: Vec<String> = labels
            .iter()
            .map(|(k, v)| format!("label={k}={v}"))
            .collect();
        let r = docker(&list_args(&filters)).await?;
        if r.code != 0 {
            bail!("docker ps failed: {}", r.text().trim());
        }
        let ids: Vec<String> = r.stdout.split_whitespace().map(String::from).collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut args = strings(["inspect", "--format", "{{.Id}} {{json .Config.Labels}}"]);
        args.extend(ids);
        let r = docker(&args).await?;
        // One removed since the listing is one fewer, not a failure.
        if r.code != 0 && !r.no_such_container() {
            bail!("docker inspect failed: {}", r.text().trim());
        }
        parse_found(&r.stdout)
    }

    async fn destroy(&self, id: &str) -> Result<()> {
        let r = docker(&strings(["rm", "--force", "--volumes", id])).await?;
        if r.code != 0 && !r.no_such_container() {
            bail!("docker rm failed: {}", r.text().trim());
        }
        Ok(())
    }
}

/// One container of a [`Docker`].
#[derive(Debug)]
pub struct DockerContainer {
    id: String,
}

impl DockerContainer {
    /// The `docker exec` arguments for `exec`. An env is named here and its
    /// value set on the `docker` process: `argv` is visible to every user of
    /// the host, and a value may be a token.
    fn exec_args(&self, exec: &Exec) -> Vec<String> {
        let mut args = vec!["exec".to_string()];
        if let Some(cwd) = &exec.cwd {
            args.extend(["--workdir".to_string(), cwd.clone()]);
        }
        for k in exec.envs.keys() {
            args.extend(["--env".to_string(), k.clone()]);
        }
        // `timeout` inside, so that a command that overruns stops there
        // rather than only here, and with the command as its own child: it
        // sends SIGKILL only while that child lives.
        let secs = exec.timeout.as_secs().max(1);
        args.extend([
            self.id.clone(),
            "sh".to_string(),
            "-c".to_string(),
            format!(
                "touch {ACTIVE} 2>/dev/null; timeout -k {KILL_AFTER_SECS} {secs} sh -c \"$1\"; \
                 r=$?; touch {ACTIVE} 2>/dev/null; exit $r"
            ),
            "clax".to_string(),
            exec.command.clone(),
        ]);
        args
    }

    /// Whether the container is running, which nothing it prints can change.
    async fn running(&self) -> Result<bool> {
        let args = strings(["inspect", "--format", "{{.State.Running}}", &self.id]);
        let r = docker(&args).await?;
        Ok(r.code == 0 && r.stdout.trim() == "true")
    }
}

#[async_trait::async_trait]
impl Container for DockerContainer {
    fn id(&self) -> &str {
        &self.id
    }

    async fn exec(&self, exec: &Exec) -> Result<ExecOutput> {
        let args = self.exec_args(exec);
        let limit = exec.timeout + EXEC_GRACE;
        let Ok(r) = tokio::time::timeout(limit, run(&args, &exec.envs)).await else {
            return Ok(ExecOutput {
                exit_code: EXIT_TIMED_OUT,
                output: "the command did not stop in time".to_string(),
            });
        };
        let r = r?;
        if r.code != 0 && !self.running().await? {
            bail!("container {} is not running: {}", self.id, r.text().trim());
        }
        Ok(ExecOutput {
            exit_code: r.code,
            output: r.text(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_docker_cli_keeps_how_it_finds_its_engine_and_no_secret() {
        let daemon = |k: &str| {
            ["DOCKER_HOST", "CLAXBOT_APP_KEY", "CLAX_DEMO_BOT_KEY"]
                .contains(&k)
                .then(|| OsString::from("x"))
        };
        let kept: Vec<&str> = kept_env(daemon).into_iter().map(|(k, _)| k).collect();
        assert_eq!(kept, ["DOCKER_HOST"]);
    }

    #[test]
    fn an_engine_on_this_machine_is_the_operators_own_and_any_other_is_not() {
        for here in [
            "unix:///var/run/docker.sock",
            "unix:///Users/a/.docker/run/docker.sock",
            "npipe:////./pipe/docker_engine",
            "tcp://127.0.0.1:2375",
            "tcp://localhost:2375",
            "tcp://[::1]:2375",
        ] {
            assert!(local_endpoint(here), "{here}");
        }
        for there in [
            "tcp://10.0.0.8:2375",
            "ssh://root@builder.example",
            "https://docker.example:2376",
            "tcp://localhost.example:2375",
            "",
            "nonsense",
        ] {
            assert!(!local_endpoint(there), "{there}");
        }
    }

    #[test]
    fn an_env_value_never_reaches_argv() {
        let c = DockerContainer { id: "c1".into() };
        let mut exec = Exec::new("true", Duration::from_secs(5)).in_dir("/w");
        exec.envs
            .insert("GIT_CONFIG_VALUE_0".into(), "Authorization: s3cret".into());
        let args = c.exec_args(&exec);
        assert!(args.iter().all(|a| !a.contains("s3cret")), "{args:?}");
        let at = args.iter().position(|a| a == "--env").unwrap();
        assert_eq!(args[at + 1], "GIT_CONFIG_VALUE_0");
        assert!(!format!("{exec:?}").contains("s3cret"));
    }

    #[test]
    fn a_command_that_ignores_sigterm_is_killed() {
        let c = DockerContainer { id: "c1".into() };
        let args = c.exec_args(&Exec::new("true", Duration::from_secs(5)));
        let wrapper = args.iter().find(|a| a.contains("timeout")).unwrap();
        assert!(wrapper.contains("timeout -k 5 5 sh -c"), "{wrapper}");
        assert!(wrapper.contains(ACTIVE), "it marks activity");
    }

    #[test]
    fn a_spec_with_an_allow_list_is_refused_before_anything_runs() {
        let spec = ContainerSpec {
            domain_allow_list: vec!["a.dev".into()],
            ..ContainerSpec::default()
        };
        let e = Docker::default().create_args(&spec, "n").unwrap_err();
        assert!(e.to_string().contains("cannot limit"));
        let open = ContainerSpec::default();
        assert!(Docker::default().create_args(&open, "n").is_ok());
    }

    #[test]
    fn a_container_is_limited_and_the_operator_has_the_last_word() {
        let d = Docker {
            run_args: strings(["--memory", "32g"]),
        };
        let args = d.create_args(&ContainerSpec::default(), "n").unwrap();
        for limit in ["--cap-drop", "--pids-limit", "no-new-privileges"] {
            assert!(args.iter().any(|a| a == limit), "{limit}");
        }
        let memory: Vec<usize> = (0..args.len()).filter(|i| args[*i] == "--memory").collect();
        assert_eq!(args[memory[memory.len() - 1] + 1], "32g");
    }

    #[test]
    fn found_containers_are_read_with_their_labels() {
        let text = "abc {\"clax.run\":\"r1\",\"x\":\"a b\"}\ndef {}\n";
        let found = parse_found(text).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, "abc");
        assert_eq!(found[0].labels["x"], "a b");
        assert!(found[1].labels.is_empty());
    }

    #[tokio::test]
    async fn output_is_kept_only_up_to_the_limit() {
        assert_eq!(read_capped(&[7u8; 100][..], 10).await.unwrap().len(), 10);
        assert_eq!(read_capped(&[7u8; 5][..], 10).await.unwrap().len(), 5);
    }

    #[test]
    fn two_streams_together_stay_under_the_limit() {
        let half = "x".repeat(MAX_EXEC_OUTPUT);
        let r = Ran {
            code: 0,
            stdout: half.clone(),
            stderr: half,
        };
        assert_eq!(r.text().len(), MAX_EXEC_OUTPUT);
    }
}

//! What every runtime must do, run against a live one or a fake of one.

use anyhow::{Result, bail, ensure};
use clax_bot_workspace::runtime::{
    Container, ContainerRuntime, ContainerSpec, Exec, MAX_EXEC_OUTPUT, is_gone,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const T: Duration = Duration::from_secs(60);

/// How long a runtime's listing, or a command's end, may lag what was done.
const SETTLE: Duration = Duration::from_secs(60);

/// The host an allow-list names, and one it does not.
const ON_LIST: &str = "example.com";
const OFF_LIST: &str = "example.org";

/// Shell text that succeeds when `host` can be reached.
pub fn reaches(host: &str) -> String {
    format!(
        "curl -sS -o /dev/null -m 15 https://{host}/ || wget -q -T 15 -O /dev/null https://{host}/"
    )
}

fn nonce() -> Result<String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string())
}

/// Fails unless `runtime`, making containers from `spec`, keeps the contract
/// of `Container` and `ContainerRuntime`. `egress` is shell text that succeeds
/// when a host can be reached, and `stop` stops a container from outside.
pub async fn check(
    runtime: &dyn ContainerRuntime,
    spec: &ContainerSpec,
    egress: fn(&str) -> String,
    stop: impl AsyncFn(&str) -> Result<()>,
) -> Result<()> {
    let mut spec = spec.clone();
    spec.labels.insert("clax.conformance".into(), nonce()?);
    let c = runtime.create(&spec).await?;
    let id = c.id().to_string();
    let checked = check_container(runtime, c.as_ref(), &spec, stop).await;
    let destroyed = runtime.destroy(&id).await;
    checked?;
    destroyed?;
    match runtime.attach(&id, &spec).await {
        Ok(_) => bail!("a destroyed container can still be attached"),
        Err(e) => ensure!(is_gone(&e), "a destroyed container is not gone: {e:#}"),
    }
    ensure!(
        c.exec(&Exec::new("true", T)).await.is_err(),
        "a destroyed container answered a command"
    );
    runtime.destroy(&id).await?;
    settled("a destroyed container is still found", async || {
        Ok(runtime.find(&spec.labels).await?.is_empty())
    })
    .await?;
    check_egress(runtime, &spec, egress).await
}

/// Waits for a condition that may lag, such as a listing: Daytona documents
/// its own as eventually consistent.
async fn settled(what: &str, holds: impl AsyncFn() -> Result<bool>) -> Result<()> {
    let deadline = Instant::now() + SETTLE;
    loop {
        if holds().await? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("{what}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// An allow-list is held or refused, never ignored, and held means a host
/// off it cannot be reached.
async fn check_egress(
    runtime: &dyn ContainerRuntime,
    spec: &ContainerSpec,
    egress: fn(&str) -> String,
) -> Result<()> {
    let mut limited = spec.clone();
    limited.domain_allow_list = vec![ON_LIST.into()];
    let c = match runtime.create(&limited).await {
        Ok(c) => c,
        Err(e) => {
            ensure!(
                !runtime.limits_domains(),
                "refused an allow-list it says it holds: {e:#}"
            );
            return Ok(());
        }
    };
    let off = c.exec(&Exec::new(egress(OFF_LIST), T)).await;
    let on = c.exec(&Exec::new(egress(ON_LIST), T)).await;
    let attached = runtime.attach(c.id(), &limited).await;
    runtime.destroy(c.id()).await?;
    ensure!(
        runtime.limits_domains(),
        "ran open under an allow-list it cannot hold"
    );
    let (off, on) = (off?, on?);
    ensure!(off.exit_code != 0, "reached a host off its list: {off:?}");
    ensure!(on.exit_code == 0, "the probe reached nothing: {on:?}");
    attached?;
    Ok(())
}

/// A value given by env does not show in the host's process list while the
/// command runs.
async fn check_env_stays_out_of_argv(c: &dyn Container) -> Result<()> {
    let canary = format!("canary-{}", nonce()?);
    let mut e = Exec::new("sleep 2; echo \"$CONFORMANCE_SECRET\"", T);
    e.envs.insert("CONFORMANCE_SECRET".into(), canary.clone());
    // Looks for as long as the command runs, and no longer.
    let running = c.exec(&e);
    tokio::pin!(running);
    let mut seen = String::new();
    let o = loop {
        let ps = tokio::process::Command::new("ps")
            .args(["-A", "-o", "args"])
            .output();
        tokio::select! {
            o = &mut running => break o?,
            ps = ps => seen.push_str(&String::from_utf8_lossy(&ps?.stdout)),
        }
    };
    ensure!(o.output.contains(&canary), "env: {o:?}");
    ensure!(!seen.contains(&canary), "an env's value was in argv");
    Ok(())
}

/// A command that overruns answers, fails, and stops even if it ignores
/// SIGTERM.
async fn check_overrun(c: &dyn Container) -> Result<()> {
    let pid = format!("/tmp/conformance-{}", nonce()?);
    let stubborn = format!("echo $$ > {pid}; trap '' TERM; sleep 300");
    let o = c.exec(&Exec::new(stubborn, Duration::from_secs(1))).await?;
    ensure!(o.exit_code != 0, "an overrun passed: {o:?}");
    settled("a command that overran is still running", async || {
        let alive = format!("kill -0 $(cat {pid})");
        Ok(c.exec(&Exec::new(alive, T)).await?.exit_code != 0)
    })
    .await
}

async fn check_container(
    runtime: &dyn ContainerRuntime,
    c: &dyn Container,
    spec: &ContainerSpec,
    stop: impl AsyncFn(&str) -> Result<()>,
) -> Result<()> {
    // Shell text is shell text.
    let o = c.exec(&Exec::new("echo 'a b' | tr a-z A-Z", T)).await?;
    ensure!(o.exit_code == 0 && o.output.trim() == "A B", "pipes: {o:?}");
    // A failing command is an answer.
    let o = c.exec(&Exec::new("echo out; exit 7", T)).await?;
    ensure!(o.exit_code == 7, "exit code: {o:?}");
    ensure!(o.output.contains("out"), "output of a failure: {o:?}");
    // stderr is output.
    let o = c.exec(&Exec::new("echo err >&2", T)).await?;
    ensure!(o.output.contains("err"), "stderr: {o:?}");
    // stdin is empty, and is not the script.
    let o = c.exec(&Exec::new("cat >/dev/null\necho after", T)).await?;
    ensure!(o.output.contains("after"), "stdin: {o:?}");
    // cwd and envs apply, to this command alone.
    let mut e = Exec::new("pwd; echo \"$CONFORMANCE\"", T).in_dir("/tmp");
    e.envs.insert("CONFORMANCE".into(), "set".into());
    let o = c.exec(&e).await?;
    ensure!(
        o.output.contains("/tmp") && o.output.contains("set"),
        "{o:?}"
    );
    let o = c.exec(&Exec::new("echo \"[$CONFORMANCE]\"", T)).await?;
    ensure!(
        o.output.contains("[]"),
        "an env outlived its command: {o:?}"
    );
    check_env_stays_out_of_argv(c).await?;
    check_overrun(c).await?;
    // Output is capped, and a command that writes too much still answers.
    let flood = format!("head -c {} /dev/zero | tr '\\0' x", MAX_EXEC_OUTPUT + 1000);
    let o = c.exec(&Exec::new(flood, T)).await?;
    ensure!(o.output.len() <= MAX_EXEC_OUTPUT, "output was not capped");
    // The filesystem persists between commands.
    c.exec(&Exec::new("echo kept > /tmp/conformance", T))
        .await?;
    let o = c.exec(&Exec::new("cat /tmp/conformance", T)).await?;
    ensure!(o.output.contains("kept"), "files: {o:?}");
    // Attaching reaches the same container.
    let again = runtime.attach(c.id(), spec).await?;
    let o = again.exec(&Exec::new("cat /tmp/conformance", T)).await?;
    ensure!(o.output.contains("kept"), "attach: {o:?}");
    // Its labels find it, and find only what carries them all.
    settled("a container's labels do not find it", async || {
        let found = runtime.find(&spec.labels).await?;
        Ok(found.len() == 1 && found[0].id == c.id() && found[0].labels_hold(&spec.labels))
    })
    .await?;
    let mut other = spec.labels.clone();
    other.insert("clax.conformance".into(), "another".into());
    ensure!(
        runtime.find(&other).await?.is_empty(),
        "labels found a container that does not carry them"
    );
    // A stopped container is out of reach, and comes back as it was.
    stop(c.id()).await?;
    ensure!(
        c.exec(&Exec::new("true", T)).await.is_err(),
        "a stopped container answered a command"
    );
    let back = runtime.attach(c.id(), spec).await?;
    let o = back.exec(&Exec::new("cat /tmp/conformance", T)).await?;
    ensure!(o.output.contains("kept"), "after a stop: {o:?}");
    Ok(())
}

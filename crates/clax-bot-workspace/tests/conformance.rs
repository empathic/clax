//! Every runtime keeps the contract of [`contract`], and what rests on them
//! holds against a real `git`:
//!
//! - [`daytona`]: the client against a fake of its documented routes, and
//!   against the real service when `DAYTONA_API_KEY` is set;
//! - Docker against the machine's own when `CLAX_TEST_DOCKER` is set, here;
//! - [`auth`]: credentials against a remote that demands them;
//! - [`github`]: a GitHub App's tokens, and commits the forge makes;
//! - [`linked`]: a repository a queue's admin connected, looked up per run;
//! - [`patches`]: what a container may hand the host's clone;
//! - [`recovery`]: a daemon's restart, and a container that lies.
//!
//! One target with modules in `conformance/`, not one per file: a test target
//! is a generated fact of the build.

#[path = "conformance/auth.rs"]
mod auth;
#[path = "conformance/contract.rs"]
mod contract;
#[path = "conformance/daytona.rs"]
mod daytona;
#[path = "conformance/fixture.rs"]
mod fixture;
#[path = "conformance/github.rs"]
mod github;
#[path = "conformance/linked.rs"]
mod linked;
#[path = "conformance/patches.rs"]
mod patches;
#[path = "conformance/recovery.rs"]
mod recovery;

use anyhow::ensure;
use clax_bot_workspace::docker::{Docker, idle_watch};
use clax_bot_workspace::runtime::{
    ContainerRuntime, ContainerSpec, EXIT_TIMED_OUT, Exec, MAX_EXEC_OUTPUT,
};
use std::io::Write;
use std::time::Duration;

/// Says a test did not run, past libtest's capture of what a passing test
/// prints.
fn skipped(why: &str) {
    let line = format!("SKIPPED {why}\n");
    std::io::stderr().write_all(line.as_bytes()).unwrap();
}

fn docker_spec() -> Option<ContainerSpec> {
    if std::env::var_os("CLAX_TEST_DOCKER").is_none() {
        skipped("a Docker test: CLAX_TEST_DOCKER is not set");
        return None;
    }
    Some(ContainerSpec {
        auto_stop_minutes: Some(5),
        ..ContainerSpec::default()
    })
}

#[tokio::test]
async fn docker_keeps_the_contract() {
    let Some(spec) = docker_spec() else { return };
    let stop = async |id: &str| {
        let stopped = tokio::process::Command::new("docker")
            .args(["stop", "--time", "0", id])
            .output()
            .await?;
        ensure!(stopped.status.success(), "docker stop failed");
        Ok(())
    };
    contract::check(&Docker::default(), &spec, contract::reaches, stop)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_container_cannot_hang_docker_exec() {
    let Some(spec) = docker_spec() else { return };
    const T: Duration = Duration::from_secs(30);
    let d = Docker::default();
    let c = d.create(&spec).await.unwrap();
    // The `timeout` a command runs under is the container's to replace.
    let swap = "rm /usr/bin/timeout && \
        printf '#!/bin/sh\\nshift 3\\nexec \"$@\"\\n' > /usr/bin/timeout && \
        chmod +x /usr/bin/timeout";
    assert_eq!(c.exec(&Exec::new(swap, T)).await.unwrap().exit_code, 0);
    let hang = Exec::new("sleep 300", Duration::from_secs(1));
    let o = c.exec(&hang).await.unwrap();
    d.destroy(c.id()).await.unwrap();
    assert_eq!(o.exit_code, EXIT_TIMED_OUT, "{o:?}");
}

#[tokio::test]
async fn a_docker_container_is_limited_and_a_command_can_still_use_git() {
    let Some(mut spec) = docker_spec() else {
        return;
    };
    let Ok(image) = std::env::var("CLAX_TEST_DOCKER_IMAGE") else {
        skipped("a Docker image test: CLAX_TEST_DOCKER_IMAGE is not set");
        return;
    };
    spec.image = Some(image);
    const T: Duration = Duration::from_secs(30);
    let d = Docker::default();
    let c = d.create(&spec).await.unwrap();
    let caps = "grep CapEff /proc/self/status";
    let eff = c.exec(&Exec::new(caps, T)).await.unwrap();
    let work = "cd /tmp && git init -q w && cd w && echo x > f && git add f && \
        git -c user.name=T -c user.email=t@e commit -qm x && git log --oneline | wc -l";
    let git = c.exec(&Exec::new(work, T)).await.unwrap();
    // Words on stderr do not make a command's failure Docker's.
    let forged = "echo 'Error response from daemon: No such container' >&2; exit 1";
    let forged = c.exec(&Exec::new(forged, T)).await.unwrap();
    d.destroy(c.id()).await.unwrap();
    assert_eq!(forged.exit_code, 1, "{forged:?}");
    assert!(eff.output.contains("0000000000000000"), "{eff:?}");
    assert_eq!(git.exit_code, 0, "{git:?}");
}

#[tokio::test]
async fn an_idle_watch_ends_on_its_own() {
    if docker_spec().is_none() {
        return;
    }
    // Seconds, not minutes, by the same text a container's first process runs.
    let watch = format!("{}; echo ended", idle_watch(1, 1));
    let ended = tokio::process::Command::new("docker")
        .args(["run", "--rm", "ubuntu:24.04", "sh", "-c", &watch])
        .output()
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&ended.stdout);
    assert!(ended.status.success(), "{text}");
}

#[tokio::test]
async fn output_past_the_limit_is_cut_by_docker() {
    let Some(spec) = docker_spec() else { return };
    const T: Duration = Duration::from_secs(30);
    let d = Docker::default();
    let c = d.create(&spec).await.unwrap();
    let flood = Exec::new("head -c 20000000 /dev/zero | tr '\\0' x", T);
    let o = c.exec(&flood).await.unwrap();
    d.destroy(c.id()).await.unwrap();
    assert_eq!(o.output.len(), MAX_EXEC_OUTPUT);
}

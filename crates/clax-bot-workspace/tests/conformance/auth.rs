//! `repo::token_env` against a remote that demands credentials: a real `git`
//! on both ends, the host's clone and a container's.

use crate::fixture::{basic, seed, serve};
use clax_bot_workspace::repo::{Clone, Token, token_env};
use std::path::Path;

const TOKEN: &str = "s3cret-token-value";

fn token() -> Token {
    Token::new(TOKEN)
}

/// Fails if `text` holds the token in any form git carries it.
fn no_token(text: &str, token: &str) {
    assert!(!text.contains(token), "the token leaked: {text}");
    let header = basic(token);
    let encoded = header.strip_prefix("Basic ").unwrap();
    assert!(!text.contains(encoded), "the token leaked: {text}");
}

#[tokio::test]
async fn the_hosts_clone_reaches_a_remote_that_demands_the_token() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let remote = serve(root.path(), TOKEN, None).await;
    let dir = root.path().join("clone");
    let clone = Clone::new(&remote.url(), &dir, Some(&token()))
        .await
        .unwrap();
    clone.start_branch("claxbot/t", "main").await.unwrap();
    assert!(crate::fixture::git(&dir, &["ls-files"]).contains("app.txt"));
    assert!(!dir.join("app.txt").exists(), "the host checks nothing out");
    assert!(!dir.join(".git/hooks").exists(), "and takes no template");
    let seen = remote.seen.lock().unwrap();
    assert!(seen.authorizations.iter().all(|a| *a == basic(TOKEN)));
    assert!(!seen.authorizations.is_empty());
}

#[tokio::test]
async fn the_hosts_clone_pushes_with_the_token() {
    let root = tempfile::tempdir().unwrap();
    let bare = seed(root.path());
    let remote = serve(root.path(), TOKEN, None).await;
    let clone = Clone::new(&remote.url(), &root.path().join("clone"), Some(&token()))
        .await
        .unwrap();
    clone.start_branch("claxbot/t", "main").await.unwrap();
    std::fs::write(root.path().join("clone/new.txt"), "new\n").unwrap();
    crate::fixture::git(&root.path().join("clone"), &["add", "new.txt"]);
    let commit = clone.commit("a change").await.unwrap();
    clone.push("claxbot/t", Some(&token())).await.unwrap();
    let pushed = crate::fixture::git(&bare, &["rev-parse", "claxbot/t"]);
    assert_eq!(pushed, commit);
}

#[tokio::test]
async fn a_wrong_or_missing_token_fails_and_the_failure_holds_no_token() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let remote = serve(root.path(), TOKEN, None).await;
    let wrong = "wr0ng-token-value";
    let e = Clone::new(
        &remote.url(),
        &root.path().join("wrong"),
        Some(&Token::new(wrong)),
    )
    .await
    .unwrap_err();
    no_token(&format!("{e:#}"), wrong);
    no_token(&format!("{e:#}"), TOKEN);
    let e = Clone::new(&remote.url(), &root.path().join("none"), None)
        .await
        .unwrap_err();
    no_token(&format!("{e:#}"), TOKEN);
}

#[tokio::test]
async fn what_a_remote_echoes_back_does_not_carry_the_token_into_an_error() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let remote = serve(root.path(), TOKEN, None).await;
    let url = format!("{}/echo/repo.git", remote.base);
    let e = Clone::new(&url, &root.path().join("c"), Some(&token()))
        .await
        .unwrap_err();
    let text = format!("{e:#}");
    assert!(text.contains("rejected"), "git showed the remote: {text}");
    no_token(&text, TOKEN);
}

#[tokio::test]
async fn a_redirect_does_not_carry_the_token_to_another_host() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    // It serves anyone, so that git would go on to its later requests.
    let elsewhere = serve(root.path(), "", None).await;
    let remote = serve(root.path(), TOKEN, Some(elsewhere.base.clone())).await;
    let url = format!("{}/moved/repo.git", remote.base);
    let e = Clone::new(&url, &root.path().join("c"), Some(&token())).await;
    assert!(e.is_err(), "a moved repository is an error");
    assert!(elsewhere.seen.lock().unwrap().authorizations.is_empty());
}

/// A container's git: no credentials but what the command's environment
/// gives it.
fn container_git(dir: &Path, url: &str, token: &str) -> std::process::Output {
    std::process::Command::new("git")
        .args(["clone", "--quiet", "--", url, "repo"])
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(token_env(&Token::new(token)))
        .output()
        .unwrap()
}

#[tokio::test]
async fn the_token_env_alone_clones_and_leaves_no_token_in_the_checkout() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let remote = serve(root.path(), TOKEN, None).await;
    let box_dir = tempfile::tempdir().unwrap();
    let url = remote.url();
    let out = tokio::task::spawn_blocking({
        let dir = box_dir.path().to_path_buf();
        move || container_git(&dir, &url, TOKEN)
    })
    .await
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let config = std::fs::read_to_string(box_dir.path().join("repo/.git/config")).unwrap();
    no_token(&config, TOKEN);
}

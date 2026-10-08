//! Git capture (spec 2026-10-06-toolpath-audit-design §9.1, §9.2) against
//! fixture repositories made with the real `git`, and a blocking fake `git`
//! for the deadline.

use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn git_bin() -> PathBuf {
    find_git(std::env::var_os("PATH").as_deref()).expect("git on PATH for the fixture tests")
}

/// The git on `PATH`, probed once.
fn real_git() -> &'static Git {
    static GIT: std::sync::OnceLock<Git> = std::sync::OnceLock::new();
    GIT.get_or_init(|| {
        let g = Git::probe(&git_bin());
        assert!(g.supported(), "the fixture tests need git 2.44 or later");
        g
    })
}

/// Runs a fixture-building git command in `dir`, isolated from the
/// machine's git configuration.
fn git(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new(git_bin())
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    out.stdout
}

/// A repository with one commit of `a.txt` on `main`.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("app");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-q", "-m", "one"]);
    let root = root.canonicalize().unwrap();
    (d, root)
}

fn soon() -> Instant {
    // Generous, so a loaded machine never turns a fixture into a timeout.
    Instant::now() + Duration::from_secs(20)
}

fn ctx(field: GitField) -> GitContext {
    match field {
        GitField::Ok(c) => c,
        other => panic!("no context: {other:?}"),
    }
}

fn sha256_of(bytes: &[u8]) -> String {
    format!("sha256:{}", crate::audit::sha256_hex(bytes))
}

/// The diff the capture hashes, run independently.
fn expected_diff(root: &Path, against: &str) -> Vec<u8> {
    let mut args = vec!["diff", against];
    args.extend(DIFF_FLAGS);
    git(root, &args)
}

#[test]
fn capture_clean_repo() {
    let (_d, root) = repo();
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://u:tok@github.com/o/r.git?x=1",
        ],
    );
    let c = ctx(capture(&root, soon(), real_git()));
    let head = String::from_utf8(git(&root, &["rev-parse", "HEAD"])).unwrap();
    assert_eq!(c.repo_root, root.to_str().unwrap());
    assert_eq!(c.branch.as_deref(), Some("main"));
    assert_eq!(c.head.as_deref(), Some(head.trim()));
    assert_eq!(c.remote.as_deref(), Some("origin"));
    assert_eq!(c.remote_url.as_deref(), Some("https://github.com/o/r.git"));
    assert!(!c.dirty);
    assert_eq!((c.diff_sha256, c.diff_bytes, c.untracked), (None, None, 0));
    assert!(is_rfc3339(&c.captured_at));
    // A subdirectory reports the repository's root.
    std::fs::create_dir(root.join("sub")).unwrap();
    let sub = ctx(capture(&root.join("sub"), soon(), real_git()));
    assert_eq!(sub.repo_root, root.to_str().unwrap());
}

#[test]
fn capture_prefers_the_upstream_remote_then_origin_then_the_first() {
    let (_d, root) = repo();
    // No remote at all.
    assert_eq!(ctx(capture(&root, soon(), real_git())).remote, None);
    git(&root, &["remote", "add", "zeta", "git@host:z/r.git"]);
    git(&root, &["remote", "add", "beta", "ssh://u:p@host/b.git"]);
    let c = ctx(capture(&root, soon(), real_git()));
    assert_eq!(c.remote.as_deref(), Some("beta"));
    assert_eq!(c.remote_url.as_deref(), Some("ssh://host/b.git"));
    git(&root, &["remote", "add", "origin", "/srv/git/app.git"]);
    let c = ctx(capture(&root, soon(), real_git()));
    assert_eq!(c.remote.as_deref(), Some("origin"));
    assert_eq!(c.remote_url.as_deref(), Some("/srv/git/app.git"));
    git(&root, &["config", "branch.main.remote", "zeta"]);
    let c = ctx(capture(&root, soon(), real_git()));
    assert_eq!(c.remote.as_deref(), Some("zeta"));
    assert_eq!(c.remote_url.as_deref(), Some("git@host:z/r.git"));
}

#[test]
fn capture_dirty_tracked_hashes_diff() {
    let (_d, root) = repo();
    std::fs::write(root.join("a.txt"), "two\n").unwrap();
    let c = ctx(capture(&root, soon(), real_git()));
    let diff = expected_diff(&root, "HEAD");
    assert!(!diff.is_empty());
    assert!(c.dirty);
    assert_eq!(c.diff_sha256, Some(sha256_of(&diff)));
    assert_eq!(c.diff_bytes, Some(diff.len() as u64));
    assert!(!c.diff_truncated);
    assert_eq!(c.untracked, 0);
}

#[test]
fn capture_staged_only() {
    let (_d, root) = repo();
    std::fs::write(root.join("a.txt"), "staged\n").unwrap();
    git(&root, &["add", "a.txt"]);
    let c = ctx(capture(&root, soon(), real_git()));
    assert!(c.dirty);
    assert_eq!(
        c.diff_sha256,
        Some(sha256_of(&expected_diff(&root, "HEAD")))
    );
}

#[test]
fn capture_untracked_only_counts_without_hash() {
    let (_d, root) = repo();
    std::fs::write(root.join("new1.txt"), "x").unwrap();
    std::fs::write(root.join("new2.txt"), "y").unwrap();
    let c = ctx(capture(&root, soon(), real_git()));
    assert!(c.dirty);
    assert_eq!(c.untracked, 2);
    assert_eq!((c.diff_sha256, c.diff_bytes), (None, None));
}

#[test]
fn capture_counts_untracked_beside_a_rename() {
    let (_d, root) = repo();
    git(&root, &["mv", "a.txt", "b.txt"]);
    std::fs::write(root.join("new.txt"), "x").unwrap();
    let c = ctx(capture(&root, soon(), real_git()));
    assert!(c.dirty);
    assert_eq!(c.untracked, 1);
    assert!(c.diff_sha256.is_some());
}

#[test]
fn capture_detached_has_no_branch() {
    let (_d, root) = repo();
    git(&root, &["checkout", "-q", "--detach"]);
    let c = ctx(capture(&root, soon(), real_git()));
    assert_eq!(c.branch, None);
    assert!(c.head.is_some());
    assert!(!c.dirty);
}

#[test]
fn capture_unborn_branch() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    git(&root, &["init", "-q"]);
    let c = ctx(capture(&root, soon(), real_git()));
    assert_eq!(c.branch.as_deref(), Some("main"));
    assert_eq!(c.head, None);
    assert!(!c.dirty);
    // A staged file is diffed against the empty tree.
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&root, &["add", "a.txt"]);
    let c = ctx(capture(&root, soon(), real_git()));
    assert!(c.dirty);
    assert_eq!(
        c.diff_sha256,
        Some(sha256_of(&expected_diff(&root, "--cached")))
    );
}

#[test]
fn capture_linked_worktree_root() {
    let (d, root) = repo();
    let wt = d.path().join("wt");
    git(
        &root,
        &["worktree", "add", "-q", "-b", "side", wt.to_str().unwrap()],
    );
    let wt = wt.canonicalize().unwrap();
    let c = ctx(capture(&wt, soon(), real_git()));
    assert_eq!(c.repo_root, wt.to_str().unwrap());
    assert_eq!(c.branch.as_deref(), Some("side"));
}

#[test]
fn capture_not_a_repo() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        capture(d.path(), soon(), real_git()),
        GitField::Capture("not-a-repo")
    );
}

#[test]
fn capture_without_a_directory_is_no_cwd() {
    assert_eq!(
        capture(Path::new(""), soon(), real_git()),
        GitField::Capture("no-cwd")
    );
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        capture(&d.path().join("gone"), soon(), real_git()),
        GitField::Capture("no-cwd")
    );
}

#[test]
fn capture_no_git_is_unavailable() {
    let (d, root) = repo();
    let missing = d.path().join("no-such-git");
    assert_eq!(
        capture(&root, soon(), &Git::assume_supported(&missing)),
        GitField::Capture("unavailable")
    );
    assert_eq!(find_git(Some(d.path().as_os_str())), None);
    assert_eq!(find_git(None), None);
}

/// A git that blocks forever from its `block` argument on, without
/// sleeping: `tail -f /dev/null` never returns.
const BLOCKING_GIT: &str = r#"#!/bin/sh
for a in "$@"; do
  case "$a" in
    "$CLAX_TEST_BLOCK_AT") exec tail -f /dev/null ;;
  esac
done
case " $* " in
  *" --show-toplevel "*) pwd -P ;;
esac
exit 0
"#;

#[test]
fn capture_times_out_at_deadline() {
    let d = tempfile::tempdir().unwrap();
    clax_fake_exe::install(&d.path().join("git"), BLOCKING_GIT);
    // The first command, and one of the concurrent ones.
    for block_at in ["rev-parse", "status"] {
        // A wrapper hands the fake the argument to block at, so this
        // process's environment is never changed.
        let wrapper = clax_fake_exe::install(
            &d.path().join(format!("git-{block_at}")),
            &format!(
                "#!/bin/sh\nCLAX_TEST_BLOCK_AT={block_at} exec \"$(dirname \"$0\")/git\" \"$@\"\n"
            ),
        );
        let started = Instant::now();
        let got = capture(
            d.path(),
            started + Duration::from_millis(300),
            &Git::assume_supported(&wrapper),
        );
        assert_eq!(got, GitField::Capture("timeout"), "{block_at}");
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "{block_at}: returned after {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn header_has_no_paths_or_contents() {
    use base64::Engine as _;
    let (_d, root) = repo();
    std::fs::write(root.join("a.txt"), "SECRET-CONTENT-tracked\n").unwrap();
    std::fs::write(root.join("secret-name.txt"), "SECRET-CONTENT-untracked").unwrap();
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://ghp_TOKEN@github.com/o/r.git",
        ],
    );
    let field = capture(&root, soon(), real_git());
    let header = encode_header(&field).unwrap();
    let sent = String::from_utf8(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&header)
            .unwrap(),
    )
    .unwrap();
    for secret in ["SECRET-CONTENT", "secret-name", "a.txt", "ghp_TOKEN", "one"] {
        assert!(!sent.contains(secret), "{secret} in {sent}");
    }
    // What is sent is the context, and it decodes as one.
    assert!(sent.contains("\"diff_sha256\":\"sha256:"), "{sent}");
    assert_eq!(decode_header(&header), field);
}

/// Measures `capture` on this repository (plan Task 12, step 3): 50 runs,
/// p50 and p95. Run by hand: `cargo test -p clax-core --release --lib --
/// --ignored capture_latency_on_this_repo --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn capture_latency_on_this_repo() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut ms: Vec<f64> = (0..50)
        .map(|_| {
            let t = Instant::now();
            let f = capture(here, t + CAPTURE_DEADLINE, real_git());
            assert!(matches!(f, GitField::Ok(_)), "{f:?}");
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    println!(
        "capture over 50 runs: p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms[24], ms[47], ms[49]
    );
}

/// A repository whose configuration names a program for every hook git
/// offers a status or a diff: a clean, smudge and process filter, a
/// textconv and an external diff, an fsmonitor and an index hook. Each
/// would leave a marker file; capture runs none of them.
#[test]
fn capture_runs_no_configured_program() {
    let (d, root) = repo();
    // Committed before any filter is configured, so its blob is its bytes.
    std::fs::write(root.join("b.txt"), "same\n").unwrap();
    git(&root, &["add", "b.txt"]);
    git(&root, &["commit", "-q", "-m", "b"]);
    let markers = d.path().join("markers");
    std::fs::create_dir(&markers).unwrap();
    let m = |name: &str| {
        format!(
            "sh -c 'touch {}; cat'",
            markers.join(name).to_str().unwrap()
        )
    };
    std::fs::write(root.join(".gitattributes"), "*.txt filter=x diff=x\n").unwrap();
    for (k, v) in [
        ("filter.x.clean", m("clean")),
        ("filter.x.smudge", m("smudge")),
        ("filter.x.process", m("process")),
        ("filter.x.required", "true".into()),
        ("filter.with.dots.and=equals.clean", m("dotted")),
        ("diff.x.textconv", m("textconv")),
        ("diff.x.command", m("diffcmd")),
        ("diff.external", m("external")),
        ("core.fsmonitor", m("fsmonitor")),
        ("core.pager", m("pager")),
    ] {
        git(&root, &["config", k, &v]);
    }
    let hooks = root.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for hook in ["post-index-change", "reference-transaction"] {
        let path = hooks.join(hook);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\ntouch {}\n",
                markers.join(hook).to_str().unwrap()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // A tracked file whose stat changed but whose content did not: a diff
    // that refreshed the index would take `index.lock`, rewrite the index
    // and run `post-index-change`.
    std::fs::File::options()
        .write(true)
        .open(root.join("b.txt"))
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    // A tracked change, whose comparison would read the file through the
    // filter.
    std::fs::write(root.join("a.txt"), "changed\n").unwrap();
    let index = root.join(".git/index");
    let state = |p: &Path| {
        (
            crate::audit::sha256_hex(&std::fs::read(p).unwrap()),
            std::fs::metadata(p).unwrap().modified().unwrap(),
        )
    };
    let before = state(&index);
    let c = ctx(capture(&root, soon(), real_git()));
    assert_eq!(state(&index), before, "capture wrote the index");
    assert!(!root.join(".git/index.lock").exists());
    assert!(c.dirty);
    assert!(c.diff_sha256.is_some());
    let ran: Vec<_> = std::fs::read_dir(&markers)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(ran.is_empty(), "capture ran {ran:?}");
    // The control: plain git runs the filter in this fixture (and fails,
    // since the filter speaks no protocol).
    let _ = Command::new(git_bin())
        .arg("-C")
        .arg(&root)
        .args(["diff", "HEAD", "--stat"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        std::fs::read_dir(&markers).unwrap().next().is_some(),
        "the fixture's filter never runs, so the test proves nothing"
    );
}

/// A repository root that is not UTF-8 is `invalid`, never recorded with
/// replacement characters.
#[test]
fn capture_refuses_a_root_that_is_not_utf8() {
    let d = tempfile::tempdir().unwrap();
    let fake = clax_fake_exe::install(
        &d.path().join("git"),
        "#!/bin/sh\ncase \" $* \" in *\" --show-toplevel \"*) printf '/tmp/\\377x\\n' ;; esac\nexit 0\n",
    );
    assert_eq!(
        capture(d.path(), soon(), &Git::assume_supported(&fake)),
        GitField::Capture("invalid")
    );
}

/// In a partial clone, a diff that needs a missing blob never fetches it
/// (which would run the remote's `uploadpack`, or ssh): the diff is
/// unavailable and the rest is captured.
#[test]
fn capture_never_fetches_in_a_partial_clone() {
    let (d, root) = repo();
    let marker = d.path().join("FETCHED");
    let bare = d.path().join("remote.git");
    git(d.path(), &["init", "-q", "--bare", bare.to_str().unwrap()]);
    let blob = String::from_utf8(git(&root, &["rev-parse", "HEAD:a.txt"])).unwrap();
    let blob = blob.trim();
    std::fs::remove_file(root.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    for (k, v) in [
        ("core.repositoryformatversion", "1".to_string()),
        ("extensions.partialClone", "origin".into()),
        ("remote.origin.url", bare.to_str().unwrap().into()),
        ("remote.origin.promisor", "true".into()),
        (
            "remote.origin.uploadpack",
            format!("sh -c 'touch {}; exit 1'", marker.to_str().unwrap()),
        ),
    ] {
        git(&root, &["config", k, &v]);
    }
    std::fs::write(root.join("a.txt"), "changed\n").unwrap();
    let c = ctx(capture(&root, soon(), real_git()));
    assert!(!marker.exists(), "capture fetched");
    assert!(c.dirty);
    assert!(c.diff_unavailable);
    assert_eq!((c.diff_sha256, c.diff_bytes), (None, None));
    assert!(c.head.is_some());
}

#[test]
fn capture_needs_git_2_44() {
    assert!(version_supported("git version 2.50.1 (Apple Git-155)"));
    assert!(version_supported("git version 2.44.0"));
    assert!(version_supported("git version 3.0.0"));
    assert!(!version_supported("git version 2.43.5"));
    assert!(!version_supported("git version 2.30.2"));
    assert!(!version_supported("git version 1.99.9"));
    assert!(!version_supported("not git"));
    assert!(!version_supported(""));
}

/// An old git, emulated by a wrapper that names an old version and, as an
/// old git would, ignores the pins and the lazy-fetch refusal, runs nothing
/// in the repository: the capture is `unavailable` after the version check
/// alone, and no fsmonitor, hook or fetch leaves its marker. The control
/// shows the wrapper does run them when asked directly.
#[test]
fn an_old_git_runs_nothing() {
    let (d, root) = repo();
    let markers = d.path().join("markers");
    std::fs::create_dir(&markers).unwrap();
    let mark = |name: &str| format!("touch {}", markers.join(name).to_str().unwrap());
    let fsmonitor = d.path().join("fsmonitor.sh");
    std::fs::write(&fsmonitor, format!("#!/bin/sh\n{}\n", mark("fsmonitor"))).unwrap();
    let hooks = root.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-index-change");
    std::fs::write(&hook, format!("#!/bin/sh\n{}\n", mark("hook"))).unwrap();
    use std::os::unix::fs::PermissionsExt;
    for f in [&fsmonitor, &hook] {
        std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let bare = d.path().join("remote.git");
    git(d.path(), &["init", "-q", "--bare", bare.to_str().unwrap()]);
    for (k, v) in [
        ("core.fsmonitor", fsmonitor.to_str().unwrap().to_string()),
        ("core.repositoryformatversion", "1".into()),
        ("extensions.partialClone", "origin".into()),
        ("remote.origin.url", bare.to_str().unwrap().into()),
        ("remote.origin.promisor", "true".into()),
        (
            "remote.origin.uploadpack",
            format!("sh -c '{}; exit 1'", mark("fetch")),
        ),
    ] {
        git(&root, &["config", k, &v]);
    }
    let blob = String::from_utf8(git(&root, &["rev-parse", "HEAD:a.txt"])).unwrap();
    let blob = blob.trim();
    std::fs::remove_file(root.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    std::fs::write(root.join("a.txt"), "changed\n").unwrap();
    let real = git_bin();
    for version in ["2.43.0", "2.30.2"] {
        let wrapper = clax_fake_exe::install(
            &d.path().join(format!("git-{version}")),
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = version ] && {{ echo 'git version {version}'; exit 0; }}; done\nunset GIT_CONFIG_COUNT GIT_NO_LAZY_FETCH\nexec {} \"$@\"\n",
                real.to_str().unwrap()
            ),
        );
        let old = Git::probe(&wrapper);
        assert!(!old.supported(), "{version}");
        assert_eq!(
            capture(&root, soon(), &old),
            GitField::Capture("unavailable"),
            "{version}"
        );
        let ran: Vec<_> = std::fs::read_dir(&markers)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(ran.is_empty(), "{version}: ran {ran:?}");
    }
    // The control: the same wrapper, run directly, runs the fsmonitor.
    let _ = Command::new(d.path().join("git-2.30.2"))
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        markers.join("fsmonitor").exists(),
        "the old-git wrapper runs nothing, so the test proves nothing"
    );
}

#[test]
fn a_moved_submodule_is_dirty() {
    let (d, root) = repo();
    let sub = d.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    git(&sub, &["init", "-q"]);
    std::fs::write(sub.join("s.txt"), "s\n").unwrap();
    git(&sub, &["add", "s.txt"]);
    git(&sub, &["commit", "-q", "-m", "s1"]);
    git(
        &root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub.to_str().unwrap(),
            "sm",
        ],
    );
    git(&root, &["commit", "-q", "-m", "sm"]);
    assert!(!ctx(capture(&root, soon(), real_git())).dirty);
    // A change inside the submodule's work tree is not looked at.
    std::fs::write(root.join("sm/s.txt"), "edited\n").unwrap();
    assert!(!ctx(capture(&root, soon(), real_git())).dirty);
    // A submodule checked out at another commit is.
    git(&root.join("sm"), &["commit", "-q", "-am", "s2"]);
    let c = ctx(capture(&root, soon(), real_git()));
    assert!(c.dirty);
    assert!(c.diff_sha256.is_some());
}

#[test]
fn diff_unavailable_only_on_a_dirty_tree() {
    let (_d, root) = repo();
    let mut c = ctx(capture(&root, soon(), real_git()));
    assert!(!c.dirty);
    c.diff_unavailable = true;
    assert!(c.validate().is_err());
    c.dirty = true;
    assert_eq!(c.validate(), Ok(()));
}

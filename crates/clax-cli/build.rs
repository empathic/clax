//! Sets `CLAX_BUILD_COMMIT` (see [`build_commit`]), so a new commit
//! recompiles this crate. The files the binary embeds are `clax-embedded`'s,
//! whose build script names them.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let repo = manifest.join("../..");
    println!("cargo:rerun-if-changed=build.rs");
    build_commit(&repo);
}

/// Sets `CLAX_BUILD_COMMIT`, the commit `clax` is built from: the
/// environment's `CLAX_BUILD_COMMIT` when set (the release workflow sets
/// it), else `HEAD` of the git repository whose root is this workspace,
/// else `unknown` (a source tarball, a crate unpacked by `cargo install`,
/// or no `git`). It depends only on the sources' commit, so builds stay
/// reproducible. The build reruns when the override changes, or when HEAD
/// or the branch it names moves (worktrees included); not on index
/// changes, so no dirty flag is recorded. Only this crate rebuilds on a
/// new commit: the binary hands the commit to the rest at startup.
fn build_commit(repo: &Path) {
    println!("cargo:rerun-if-env-changed=CLAX_BUILD_COMMIT");
    let commit = match std::env::var("CLAX_BUILD_COMMIT") {
        Ok(c) if !c.is_empty() => {
            assert!(
                c == "unknown" || is_commit(&c),
                "CLAX_BUILD_COMMIT must be 7 to 64 lowercase hex digits or `unknown`"
            );
            c
        }
        _ => git_head(repo).unwrap_or_else(|| "unknown".into()),
    };
    println!("cargo:rustc-env=CLAX_BUILD_COMMIT={commit}");
}

/// Whether `c` is a commit name: 7 to 64 lowercase hex digits.
fn is_commit(c: &str) -> bool {
    (7..=64).contains(&c.len())
        && c.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `git` run in `dir` with `args`: its trimmed stdout on success.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `HEAD` of the repository whose root is `repo`, watching what moves it. A
/// repository that merely encloses the sources (a crate unpacked under a
/// home directory kept in git, say) is not this one.
fn git_head(repo: &Path) -> Option<String> {
    let root = repo.canonicalize().ok()?;
    let top = PathBuf::from(git(&root, &["rev-parse", "--show-toplevel"])?);
    if top.canonicalize().ok()? != root {
        return None;
    }
    let head = git(&root, &["rev-parse", "HEAD"]).filter(|c| is_commit(c))?;
    let path = |name: &str| git(&root, &["rev-parse", "--git-path", name]).map(|p| root.join(p));
    let head_file = path("HEAD")?;
    println!("cargo:rerun-if-changed={}", head_file.display());
    // The branch HEAD names moves by its loose ref file, which a commit
    // writes even when the branch was packed: so the nearest existing
    // directory on that file's path is watched (a directory reruns on any
    // change below it), and the packed refs. A missing watched path would
    // count as changed on every build.
    if let Some(name) = std::fs::read_to_string(&head_file)
        .ok()
        .and_then(|h| h.trim().strip_prefix("ref: ").map(str::to_string))
        && let Some(loose) = path(&name)
    {
        if let Some(dir) = loose.ancestors().skip(1).find(|d| d.is_dir()) {
            println!("cargo:rerun-if-changed={}", dir.display());
        }
        if let Some(packed) = path("packed-refs").filter(|p| p.exists()) {
            println!("cargo:rerun-if-changed={}", packed.display());
        }
    }
    Some(head)
}

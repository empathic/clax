//! The `clax` binary the tests run, and the repository root.
//!
//! Under Cargo, `CARGO_MANIFEST_DIR` is absolute; under Bazel it is the
//! crate's path from the workspace root, and tests start in the runfiles
//! root, which then stands for the repository. Paths are made absolute once,
//! at first use, so a test that hands one to a child running elsewhere hands
//! it a path that still resolves.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The repository root for a crate whose manifest directory is `manifest`
/// (two levels below the root, `crates/<name>`), when the process started
/// in `cwd`.
pub fn repo_from(manifest: &Path, cwd: &Path) -> PathBuf {
    if manifest.is_absolute() {
        normalize(&manifest.join("../.."))
    } else {
        cwd.to_path_buf()
    }
}

/// `path`, absolute against `cwd` when it is relative.
pub fn absolute_from(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        normalize(&cwd.join(path))
    }
}

/// `path` with `.` and `..` components removed lexically.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The repository root, absolute.
pub fn repo() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let cwd = std::env::current_dir().expect("a current directory");
        repo_from(Path::new(env!("CARGO_MANIFEST_DIR")), &cwd)
    })
    .clone()
}

/// Where the `clax` the tests run comes from.
#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    /// `CLAX_TEST_BIN`, run as it is (the gates and Bazel set it, to a
    /// binary run once already).
    Given(PathBuf),
    /// Cargo's build for this test (`CARGO_BIN_EXE_clax`, which only
    /// clax-cli's tests have), to keep by content.
    Built(PathBuf),
    /// Nothing named: build clax-cli, then keep it by content.
    Build,
}

/// The choice [`clax_bin`] makes from `CLAX_TEST_BIN` (`test_bin`; empty
/// counts as unset) and Cargo's `CARGO_BIN_EXE_clax` (`cargo_bin_exe`).
/// Relative paths are against the repository root `root`, which under
/// Bazel is the start directory.
pub fn choose(test_bin: Option<OsString>, cargo_bin_exe: Option<&str>, root: &Path) -> Source {
    match (test_bin.filter(|v| !v.is_empty()), cargo_bin_exe) {
        (Some(bin), _) => Source::Given(absolute_from(PathBuf::from(bin), root)),
        (None, Some(built)) => Source::Built(absolute_from(PathBuf::from(built), root)),
        (None, None) => Source::Build,
    }
}

/// The `clax` the tests run, absolute, chosen by [`choose`] once per
/// process. A Cargo-built binary is kept by content (`scripts/stable-bin.sh`)
/// and run once there, so no test pays macOS's first-run assessment and no
/// later build replaces it under a run; see [`keep`] for what that costs.
pub fn clax_bin(cargo_bin_exe: Option<&'static str>) -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let root = repo();
        let built = match choose(std::env::var_os("CLAX_TEST_BIN"), cargo_bin_exe, &root) {
            Source::Given(bin) => {
                assert!(
                    bin.is_file(),
                    "CLAX_TEST_BIN {} is not a file",
                    bin.display()
                );
                return bin;
            }
            Source::Built(built) => built,
            Source::Build => cargo_build(&root),
        };
        assert!(built.is_file(), "{} missing", built.display());
        // <target>/<profile>/clax keeps its copies in <target>/clax-bin/test.
        let store = built
            .parent()
            .and_then(Path::parent)
            .expect("<target>/<profile>/clax")
            .join("clax-bin/test");
        keep(&built, &store, &root)
    })
    .clone()
}

/// `cargo build -p clax-cli` into the target directory and profile of the
/// running test executable, which must sit where a plain `cargo test` or
/// `cargo nextest run` puts it, `<target>/<debug|release>/deps/`: under
/// `--target` or a custom profile the binary's place is not known from
/// here, so the build refuses and asks for `CLAX_TEST_BIN`.
fn cargo_build(root: &Path) -> PathBuf {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let exe = std::env::current_exe().expect("the test executable");
    let deps = exe.parent();
    let target = deps.and_then(Path::parent).and_then(Path::parent);
    let target = match (deps, target) {
        (Some(deps), Some(target))
            if deps.file_name() == Some("deps".as_ref())
                && deps.parent().and_then(Path::file_name) == Some(profile.as_ref())
                && target.join("CACHEDIR.TAG").is_file() =>
        {
            target.to_path_buf()
        }
        _ => panic!(
            "{} is not in <target>/{profile}/deps/, where the clax tests build clax \
             (a --target or a custom profile moves it): set CLAX_TEST_BIN to the clax to run",
            exe.display()
        ),
    };
    let mut cmd = std::process::Command::new(env!("CARGO"));
    cmd.current_dir(root)
        .args(["build", "--quiet", "-p", "clax-cli", "--bin", "clax"])
        .arg("--target-dir")
        .arg(&target);
    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    let status = cmd.status().expect("run cargo build");
    assert!(status.success(), "cargo build -p clax-cli failed");
    target.join(profile).join("clax")
}

/// The copy of `built` kept by content in `store`, made by
/// `scripts/stable-bin.sh` (under the repository root `root`) at most once
/// per build. A pointer `<store>/by-stat/<dev>-<ino>-<size>-<mtime_ns>`
/// names the copy kept for the file `built` is now; Cargo writes a new
/// file at each link, so the key changes with the bytes. On a miss, one
/// process at a time (an exclusive lock on `<store>/.lock`) copies, hashes
/// and runs the binary, and writes the pointer only when `built` was the
/// same file after the copy as before it.
pub fn keep(built: &Path, store: &Path, root: &Path) -> PathBuf {
    let pointers = store.join("by-stat");
    let key = stat_key(built);
    if let Some(kept) = pointed(&pointers.join(&key)) {
        touch(&kept);
        return kept;
    }
    std::fs::create_dir_all(&pointers).expect("make the kept clax store");
    let lock = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store.join(".lock"))
        .expect("open the kept clax store's lock");
    lock.lock().expect("lock the kept clax store");
    if let Some(kept) = pointed(&pointers.join(&key)) {
        touch(&kept);
        return kept;
    }
    let out = std::process::Command::new(root.join("scripts/stable-bin.sh"))
        .arg(built)
        .arg(store)
        .output()
        .expect("run scripts/stable-bin.sh");
    assert!(
        out.status.success(),
        "stable-bin.sh: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let kept = String::from_utf8(out.stdout).expect("a UTF-8 path");
    let kept = absolute_from(PathBuf::from(kept.trim()), root);
    // stable-bin.sh may have pruned (or this may be the first look at) the
    // store's directory: pointers to removed copies go, and theirs is made.
    let _ = std::fs::create_dir_all(&pointers);
    if let Ok(entries) = std::fs::read_dir(&pointers) {
        for e in entries.flatten() {
            if pointed(&e.path()).is_none() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    if stat_key(built) == key {
        let tmp = pointers.join(format!(".new.{}", std::process::id()));
        let written = std::fs::write(&tmp, kept.as_os_str().as_encoded_bytes())
            .and_then(|()| std::fs::rename(&tmp, pointers.join(&key)));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    kept
}

/// The file `path` names: device, inode, size and modification time.
fn stat_key(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).expect("stat the built clax");
    let mtime_ns = i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec());
    format!("{}-{}-{}-{}", m.dev(), m.ino(), m.size(), mtime_ns)
}

/// The kept copy a pointer file names, while that copy is there.
fn pointed(pointer: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(pointer).ok()?;
    let kept = PathBuf::from(OsString::from_vec(bytes));
    kept.is_file().then_some(kept)
}

/// Marks the kept copy's directory used, as `stable-bin.sh` does, so its
/// pruning keeps a copy runs still take.
fn touch(kept: &Path) {
    if let Some(dir) = kept.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.set_modified(std::time::SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_manifest_names_the_root_two_levels_up() {
        assert_eq!(
            repo_from(
                Path::new("/r/crates/clax-fake-exe"),
                Path::new("/elsewhere")
            ),
            PathBuf::from("/r")
        );
    }

    #[test]
    fn a_relative_manifest_means_the_start_directory_is_the_root() {
        assert_eq!(
            repo_from(Path::new("crates/clax-fake-exe"), Path::new("/run/_main")),
            PathBuf::from("/run/_main")
        );
    }

    #[test]
    fn a_relative_binary_is_made_absolute_against_the_start_directory() {
        assert_eq!(
            absolute_from(
                PathBuf::from("crates/clax-cli/clax"),
                Path::new("/run/_main")
            ),
            PathBuf::from("/run/_main/crates/clax-cli/clax")
        );
        assert_eq!(
            absolute_from(PathBuf::from("/abs/clax"), Path::new("/run")),
            PathBuf::from("/abs/clax")
        );
    }

    #[test]
    fn a_set_clax_test_bin_wins_over_cargos_build() {
        let root = Path::new("/r");
        assert_eq!(
            choose(Some("/given/clax".into()), Some("/t/debug/clax"), root),
            Source::Given(PathBuf::from("/given/clax"))
        );
        assert_eq!(
            choose(Some("/given/clax".into()), None, root),
            Source::Given(PathBuf::from("/given/clax"))
        );
    }

    #[test]
    fn a_relative_clax_test_bin_is_against_the_repository_root() {
        assert_eq!(
            choose(
                Some("target/clax-bin/debug/ab/clax".into()),
                Some("/t/debug/clax"),
                Path::new("/r")
            ),
            Source::Given(PathBuf::from("/r/target/clax-bin/debug/ab/clax"))
        );
    }

    #[test]
    fn an_empty_clax_test_bin_counts_as_unset() {
        let root = Path::new("/r");
        assert_eq!(
            choose(Some("".into()), Some("/t/debug/clax"), root),
            Source::Built(PathBuf::from("/t/debug/clax"))
        );
        assert_eq!(choose(Some("".into()), None, root), Source::Build);
        assert_eq!(choose(None, None, root), Source::Build);
    }

    #[test]
    fn a_kept_copy_named_by_the_sources_stat_is_used_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let built = dir.path().join("clax");
        std::fs::write(&built, "not run").unwrap();
        let kept = dir.path().join("kept/clax");
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, "not run either").unwrap();
        let store = dir.path().join("store");
        std::fs::create_dir_all(store.join("by-stat")).unwrap();
        std::fs::write(
            store.join("by-stat").join(stat_key(&built)),
            kept.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        // The root has no scripts/stable-bin.sh: a miss would panic.
        assert_eq!(keep(&built, &store, dir.path()), kept);
    }

    #[test]
    fn a_pointer_to_a_removed_copy_is_no_hit() {
        let dir = tempfile::tempdir().unwrap();
        let pointer = dir.path().join("p");
        std::fs::write(
            &pointer,
            dir.path().join("gone").as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        assert_eq!(pointed(&pointer), None);
        assert_eq!(pointed(&dir.path().join("missing")), None);
    }

    #[test]
    fn a_new_file_at_the_same_path_has_a_new_key() {
        let dir = tempfile::tempdir().unwrap();
        let built = dir.path().join("clax");
        std::fs::write(&built, "one").unwrap();
        let before = stat_key(&built);
        // As a relink does: a new file renamed over the old one.
        std::fs::write(dir.path().join("next"), "one").unwrap();
        std::fs::rename(dir.path().join("next"), &built).unwrap();
        assert_ne!(stat_key(&built), before);
    }
}

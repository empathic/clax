//! The `clax` binary the tests run, and the repository root.
//!
//! Under Cargo, `CARGO_MANIFEST_DIR` is absolute; under Bazel it is the
//! crate's path from the workspace root, and tests start in the runfiles
//! root, which then stands for the repository. Paths are made absolute once,
//! at first use, so a test that hands one to a child running elsewhere hands
//! it a path that still resolves.

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

/// The `clax` the tests run, absolute: `CLAX_TEST_BIN` when set (the gates
/// and Bazel set it, to a binary run once already); else `cargo_bin_exe`
/// (Cargo's `CARGO_BIN_EXE_clax`, which only clax-cli's tests have); else a
/// `cargo build -p clax-cli` into the target directory of the running test
/// executable. A Cargo-built binary is kept by content
/// (`scripts/stable-bin.sh`) and run once there, so no test pays macOS's
/// first-run assessment and no later build replaces it under a run.
/// Computed once per process.
pub fn clax_bin(cargo_bin_exe: Option<&'static str>) -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let cwd = std::env::current_dir().expect("a current directory");
        if let Some(bin) = std::env::var_os("CLAX_TEST_BIN") {
            let bin = absolute_from(PathBuf::from(bin), &cwd);
            assert!(bin.exists(), "CLAX_TEST_BIN {} missing", bin.display());
            return bin;
        }
        let root = repo();
        let built = match cargo_bin_exe {
            Some(p) => absolute_from(PathBuf::from(p), &cwd),
            None => {
                let mut cmd = std::process::Command::new(env!("CARGO"));
                cmd.current_dir(&root)
                    .args(["build", "--quiet", "-p", "clax-cli", "--bin", "clax"]);
                if !cfg!(debug_assertions) {
                    cmd.arg("--release");
                }
                let status = cmd.status().expect("run cargo build");
                assert!(status.success(), "cargo build -p clax-cli failed");
                // Cargo's layout: the test executable lives in
                // <target>/<profile>/deps/, and `cargo build` (which honours
                // the target directory of the enclosing `cargo test`) puts
                // the binary in <target>/<profile>/.
                let exe = std::env::current_exe().expect("the test executable");
                exe.parent()
                    .and_then(Path::parent)
                    .expect("<target>/<profile>/deps/<test>")
                    .join("clax")
            }
        };
        assert!(built.exists(), "{} missing", built.display());
        // <target>/<profile>/clax keeps its copies in <target>/clax-bin/test.
        let store = built
            .parent()
            .and_then(Path::parent)
            .expect("<target>/<profile>/clax")
            .join("clax-bin/test");
        let out = std::process::Command::new(root.join("scripts/stable-bin.sh"))
            .arg(&built)
            .arg(&store)
            .output()
            .expect("run scripts/stable-bin.sh");
        assert!(
            out.status.success(),
            "stable-bin.sh: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let kept = String::from_utf8(out.stdout).expect("a UTF-8 path");
        absolute_from(PathBuf::from(kept.trim()), &root)
    })
    .clone()
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
}

//! Helpers shared by the test modules.

use std::path::PathBuf;
use std::sync::OnceLock;

/// The `clax` binary: `CLAX_TEST_BIN` when set, else built once per test
/// process. It belongs to `clax-cli`, so Cargo does not provide
/// `CARGO_BIN_EXE_clax` here; the tests build it into the same target
/// directory as this test executable. Under cargo nextest, where each test
/// is a process of its own, set `CLAX_TEST_BIN` (`just test` and
/// quality_gates.sh do): each process's build would replace
/// target/debug/clax while other tests run it.
pub fn clax_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        // A prebuilt binary (quality_gates.sh builds one for the whole run):
        // each test process would otherwise run its own `cargo build`.
        if let Some(bin) = std::env::var_os("CLAX_TEST_BIN") {
            let bin = PathBuf::from(bin);
            assert!(bin.exists(), "CLAX_TEST_BIN {} missing", bin.display());
            return bin;
        }
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut cmd = std::process::Command::new(env!("CARGO"));
        cmd.current_dir(&workspace)
            .args(["build", "--quiet", "-p", "clax-cli", "--bin", "clax"]);
        if !cfg!(debug_assertions) {
            cmd.arg("--release");
        }
        let status = cmd.status().expect("run cargo build");
        assert!(status.success(), "cargo build -p clax-cli failed");
        // Assumes Cargo's layout: the test executable lives in
        // <target>/<profile>/deps/, and `cargo build` (which honours
        // CARGO_TARGET_DIR like the enclosing `cargo test`) puts the binary in
        // <target>/<profile>/.
        let exe = std::env::current_exe().unwrap();
        let bin = exe.parent().unwrap().parent().unwrap().join("clax");
        assert!(bin.exists(), "{} missing", bin.display());
        bin
    })
    .clone()
}

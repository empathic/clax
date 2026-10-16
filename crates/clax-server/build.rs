//! Names the web UI's directory for the rust-embed of `routes/shell.rs`:
//! `CLAX_WEB_DIST_DIR` from the environment when set (Bazel sets it to its
//! build of the web UI), else the checkout's `web/dist`.

fn main() {
    println!("cargo::rerun-if-env-changed=CLAX_WEB_DIST_DIR");
    let dir = std::env::var("CLAX_WEB_DIST_DIR")
        .unwrap_or_else(|_| format!("{}/../../web/dist/", env!("CARGO_MANIFEST_DIR")));
    println!("cargo::rustc-env=CLAX_WEB_DIST_DIR={dir}");
}

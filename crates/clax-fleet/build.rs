//! Sets `CLAX_BUILD_COMMIT`, the commit `clax-fleet` is built from, which
//! `/status` reports: the build environment's `CLAX_BUILD_COMMIT` when set
//! (the image build passes it, as the build context holds no `.git`), else
//! `unknown`. It reruns only when that variable changes, so builds stay
//! reproducible.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CLAX_BUILD_COMMIT");
    let commit = match std::env::var("CLAX_BUILD_COMMIT") {
        Ok(c) if !c.is_empty() => {
            assert!(
                c == "unknown" || is_commit(&c),
                "CLAX_BUILD_COMMIT must be 7 to 64 lowercase hex digits or `unknown`"
            );
            c
        }
        _ => "unknown".into(),
    };
    println!("cargo:rustc-env=CLAX_BUILD_COMMIT={commit}");
}

/// Whether `c` is a commit name: 7 to 64 lowercase hex digits.
fn is_commit(c: &str) -> bool {
    (7..=64).contains(&c.len())
        && c.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

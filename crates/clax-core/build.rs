//! Sets `CLAX_EXTENSION_PUBLIC_KEY` to the committed public key of the Clax
//! Chrome extension (`web/extension/key/key.pub.b64`, one line of base64
//! SubjectPublicKeyInfo DER) when that file exists, so that
//! `clax_core::extension::PUBLIC_KEY` fixes the extension's ID. Without the
//! file the variable is unset and the ID comes from the install path.
//!
//! The key lives alone in `web/extension/key/`, which is always present, and
//! that directory is what is watched: Cargo counts a missing watched file as
//! changed on every build, and watching `web/extension` would rebuild on
//! every edit of the extension's sources.

use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/extension/key");
    println!("cargo::rerun-if-changed={}", dir.display());
    if let Ok(text) = std::fs::read_to_string(dir.join("key.pub.b64")) {
        let text = text.trim();
        if !text.is_empty() {
            println!("cargo::rustc-env=CLAX_EXTENSION_PUBLIC_KEY={text}");
        }
    }
}

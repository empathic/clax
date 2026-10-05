//! Sets `CLAX_EXTENSION_PUBLIC_KEY` to the committed public key of the Clax
//! Chrome extension (`web/extension/key.pub.b64`, one line of base64
//! SubjectPublicKeyInfo DER) when that file exists, so that
//! `clax_core::extension::PUBLIC_KEY` fixes the extension's ID. Without the
//! file the variable is unset and the ID comes from the install path.

use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/extension");
    let key = dir.join("key.pub.b64");
    match std::fs::read_to_string(&key) {
        Ok(text) => {
            println!("cargo::rerun-if-changed={}", key.display());
            let text = text.trim();
            if !text.is_empty() {
                println!("cargo::rustc-env=CLAX_EXTENSION_PUBLIC_KEY={text}");
            }
        }
        // Cargo counts a watched file that is missing as changed on every
        // build, so while there is no key the directory that would hold it is
        // watched instead.
        Err(_) => println!("cargo::rerun-if-changed={}", dir.display()),
    }
}

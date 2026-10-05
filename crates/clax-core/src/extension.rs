//! The Clax Chrome extension's identity and credentials (spec
//! 2026-10-05-chrome-overlay-design L15, §5.3, §9). The extension's ID is the
//! ID in effect for a Clax home: the committed public key's when there is
//! one, else the one Chromium gives the unpacked extension at
//! `<home>/extension`. The daemon admits that origin only.

use base64::Engine as _;
use rand::RngCore as _;
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// The extension's manifest, as the repository holds it (no `key`).
pub const MANIFEST: &str = include_str!("../../../web/extension/manifest.json");
/// The committed public key (`web/extension/key/key.pub.b64`), when there is one;
/// `build.rs` sets `CLAX_EXTENSION_PUBLIC_KEY` from it.
pub const PUBLIC_KEY: Option<&str> = option_env!("CLAX_EXTENSION_PUBLIC_KEY");
/// The native messaging host's name.
pub const HOST_NAME: &str = "dev.empathic.clax";
/// Every credential starts with this.
pub const CREDENTIAL_PREFIX: &str = "cxe_";
/// A credential unused for this long is no longer accepted.
pub const CREDENTIAL_TTL_DAYS: i64 = 30;
/// Live credentials kept per extension ID; minting past it revokes the oldest.
pub const MAX_CREDENTIALS: usize = 8;

/// `chrome-extension://<id>`, the `Origin` the extension with ID `id` sends.
pub fn extension_origin(id: &str) -> String {
    format!("chrome-extension://{id}")
}

/// Chromium's ID for bytes it hashes: the first 128 bits of their SHA-256,
/// each nibble written as a letter `a`–`p`.
fn id_of_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)[..16]
        .iter()
        .flat_map(|b| [b >> 4, b & 0xf])
        .map(|n| char::from(b'a' + n))
        .collect()
}

/// Chromium's ID for an unpacked extension with no `key`, given the bytes of
/// the (canonical, absolute) path it loaded.
pub fn extension_id_of_path_bytes(path: &[u8]) -> String {
    id_of_bytes(path)
}

/// Chromium's ID for an unpacked extension loaded from `dir` with no `key`.
/// Chromium hashes the path it loaded, with symlinks resolved, so this hashes
/// `dir` made absolute with its nearest existing ancestor canonicalized and
/// the rest appended: the ID is the same before and after `dir` exists (on
/// macOS a temporary `/var/…` home is `/private/var/…` to Chromium).
pub fn extension_id_from_path(dir: &Path) -> String {
    extension_id_of_path_bytes(
        canonical_as_far_as_exists(dir)
            .as_os_str()
            .as_encoded_bytes(),
    )
}

/// `dir` made absolute, with its nearest existing ancestor canonicalized and
/// the rest appended with `.` and `..` resolved lexically.
fn canonical_as_far_as_exists(dir: &Path) -> PathBuf {
    let abs = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    for a in abs.ancestors() {
        if let Ok(mut out) = a.canonicalize() {
            for part in abs.strip_prefix(a).unwrap_or(Path::new("")).components() {
                match part {
                    Component::ParentDir => {
                        out.pop();
                    }
                    Component::Normal(p) => out.push(p),
                    _ => {}
                }
            }
            return out;
        }
    }
    abs
}

/// Chromium's extension ID for a manifest `key` (base64 SubjectPublicKeyInfo
/// DER): the same rule applied to the key's bytes. `None` when `key_b64` is
/// not base64.
pub fn extension_id_from_key(key_b64: &str) -> Option<String> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(key_b64.trim())
        .ok()?;
    Some(id_of_bytes(&der))
}

/// The ID in effect for the Clax home at `home`: the committed public key's,
/// else the one Chromium gives `<home>/extension` unpacked.
pub fn extension_id_in_effect(home: &Path) -> String {
    PUBLIC_KEY
        .and_then(extension_id_from_key)
        .unwrap_or_else(|| extension_id_from_path(&home.join("extension")))
}

/// A new credential: the prefix and 32 random bytes in unpadded base64url.
pub fn new_credential() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!(
        "{CREDENTIAL_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

/// A credential's SHA-256 in lowercase hex: all the store and the daemon keep.
pub fn credential_hash(c: &str) -> String {
    Sha256::digest(c.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `c` has a credential's shape (the prefix and 43 base64url characters).
pub fn is_credential(c: &str) -> bool {
    c.strip_prefix(CREDENTIAL_PREFIX).is_some_and(|r| {
        r.len() == 43
            && r.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_hashes_to_chromiums_unpacked_id() {
        assert_eq!(
            extension_id_of_path_bytes(b"/Users/alex/.clax/extension"),
            "bhhldgpcjhfhmcfjjnelbbdcefnocaln"
        );
    }

    #[test]
    fn a_key_hashes_to_chromiums_id() {
        assert_eq!(
            extension_id_from_key("AAAA").as_deref(),
            Some("hajoiamiieihkcebbobooenpljpcckig")
        );
        assert_eq!(extension_id_from_key("not base64!"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_path_id_canonicalizes_the_existing_part_whether_or_not_the_rest_exists() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let want =
            extension_id_of_path_bytes(real.join("ax/extension").as_os_str().as_encoded_bytes());
        assert_eq!(extension_id_from_path(&link.join("ax/extension")), want);
        assert_eq!(
            extension_id_from_path(&link.join("ax/missing/../extension")),
            want,
            "a `..` in the part that does not exist yet is resolved too"
        );
        std::fs::create_dir_all(real.join("ax/extension")).unwrap();
        assert_eq!(extension_id_from_path(&link.join("ax/extension")), want);
    }

    #[test]
    fn the_id_in_effect_is_the_keys_else_the_home_extension_paths() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("ax");
        let id = extension_id_in_effect(&home);
        match PUBLIC_KEY {
            Some(k) => assert_eq!(Some(id.clone()), extension_id_from_key(k)),
            None => assert_eq!(id, extension_id_from_path(&home.join("extension"))),
        }
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| (b'a'..=b'p').contains(&b)), "{id}");
        assert_eq!(extension_origin(&id), format!("chrome-extension://{id}"));
    }

    #[test]
    fn the_manifest_declares_no_key_and_no_host_permissions() {
        let m: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
        assert_eq!(m["manifest_version"], 3);
        assert!(m.get("key").is_none());
        assert!(m.get("host_permissions").is_none());
        assert!(m.get("content_scripts").is_none());
    }

    #[test]
    fn credentials_have_their_shape_and_hash_without_revealing_themselves() {
        let c = new_credential();
        assert!(is_credential(&c), "{c}");
        assert_eq!(c.len(), 4 + 43);
        assert_ne!(new_credential(), c);
        let h = credential_hash(&c);
        assert_eq!(h.len(), 64);
        assert!(
            h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert!(!h.contains(&c[4..]));
        assert!(!is_credential("cxe_short"));
        assert!(!is_credential("Bearer x"));
        assert!(!is_credential(&format!("cxe_{}", "+".repeat(43))));
    }
}

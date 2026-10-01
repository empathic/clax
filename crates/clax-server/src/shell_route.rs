//! The shell's URL scheme, read on the daemon as `web/shell/src/route.ts`
//! reads it in the browser (`route-cases.json` pins that they agree).

use clax_core::ArtifactId;

/// What a shell path names.
#[derive(Debug, PartialEq)]
pub enum ShellRoute {
    Gallery,
    /// `/a/<id>[/v/<n>][/<file>]`; `file` is `index.html` when the path names none.
    Artifact {
        id: ArtifactId,
        version: Option<u64>,
        file: String,
    },
}

/// `path` (still percent-encoded) as the shell reads it: after `/a/<id>`, a
/// `v` segment followed by an all-digit one is a version and the rest is the
/// file, each segment decoded; anything malformed is the gallery.
pub fn parse_shell_path(path: &str) -> ShellRoute {
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() < 3 || !segs[0].is_empty() || segs[1] != "a" {
        return ShellRoute::Gallery;
    }
    let Ok(id) = ArtifactId::parse(segs[2]) else {
        return ShellRoute::Gallery;
    };
    let mut rest = &segs[3..];
    let mut version = None;
    if rest.len() >= 2
        && rest[0] == "v"
        && !rest[1].is_empty()
        && rest[1].bytes().all(|b| b.is_ascii_digit())
    {
        // Too many digits for u64 reads as u64::MAX, a version no artifact has.
        version = Some(rest[1].parse::<u64>().unwrap_or(u64::MAX));
        rest = &rest[2..];
    }
    if rest.last() == Some(&"") {
        rest = &rest[..rest.len() - 1];
    }
    let mut parts = Vec::with_capacity(rest.len());
    for s in rest {
        let Some(d) = decode_component(s) else {
            return ShellRoute::Gallery;
        };
        parts.push(d);
    }
    let file = parts.join("/");
    ShellRoute::Artifact {
        id,
        version,
        file: if file.is_empty() {
            "index.html".into()
        } else {
            file
        },
    }
}

/// JavaScript's `decodeURIComponent`: `%XX` escapes (two hex digits each)
/// decoded to bytes that must form UTF-8; `None` where it would throw.
pub fn decode_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = b.get(i + 1..i + 3)?;
            if !h.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agrees_with_the_shell_on_every_shared_case() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../web/shell/src/route-cases.json"
        )))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let path = case["path"].as_str().unwrap();
            let want = &case["route"];
            let got = match parse_shell_path(path) {
                ShellRoute::Gallery => serde_json::json!({"kind": "gallery"}),
                ShellRoute::Artifact { id, version, file } => serde_json::json!({
                    "kind": "artifact", "id": id.as_str(), "version": version, "file": file
                }),
            };
            assert_eq!(&got, want, "{path}");
        }
    }

    #[test]
    fn decodes_like_decode_uri_component() {
        assert_eq!(decode_component("a%20b").as_deref(), Some("a b"));
        assert_eq!(decode_component("%2F").as_deref(), Some("/"));
        assert_eq!(decode_component("%zz"), None);
        assert_eq!(decode_component("%+1"), None);
        assert_eq!(decode_component("%"), None);
        assert_eq!(decode_component("%C3"), None);
    }
}

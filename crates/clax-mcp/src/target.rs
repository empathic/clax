//! What a tool's `url_or_id` names (spec 2026-10-05 §6.2): an artifact (its
//! ID, or a URL of this daemon's) or a page (any other http(s) URL).

use crate::render;
use crate::tools::artifact_ref;
use rmcp::model::CallToolResult;
use serde_json::json;

/// What a tool's `url_or_id` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An artifact, and the version when the URL names one.
    Artifact { id: String, version: Option<u32> },
    /// A web page, by its URL as given.
    Page(String),
}

fn invalid(s: &str) -> CallToolResult {
    render::error(
        "invalid_id",
        format!("'{s}' is not an artifact ID, a Clax URL, or an http(s) page URL"),
        json!({}),
    )
}

/// What `url_or_id` names, for a daemon reached at `daemon_base`. An
/// http(s) URL on the daemon's port is an artifact reference (whatever its
/// host: `localhost`, `127.0.0.1`, a LAN address, or `<id>.localhost`); any
/// other http(s) URL is a page. An artifact reference, and text without a
/// scheme, are read as [`artifact_ref`] reads them: an ID, or a Clax path
/// such as `localhost:7480/a/<id>` or `/c/<id>/v/<n>/…`.
///
/// # Errors
/// `invalid_id` for anything else, including a URL on the daemon's port
/// that names no artifact and a URL of another scheme.
pub fn target(url_or_id: &str, daemon_base: &str) -> Result<Target, CallToolResult> {
    let s = url_or_id.trim();
    if s.contains("://") {
        let u = url::Url::parse(s).map_err(|_| invalid(s))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(invalid(s));
        }
        let daemon_port = url::Url::parse(daemon_base)
            .ok()
            .and_then(|d| d.port_or_known_default());
        if daemon_port.is_none() || u.port_or_known_default() != daemon_port {
            return Ok(Target::Page(s.to_string()));
        }
    }
    artifact_ref(s)
        .map(|(id, version)| Target::Artifact { id, version })
        .map_err(|_| invalid(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: &str = "http://localhost:7480";

    fn art(s: &str) -> Option<(String, Option<u32>)> {
        match target(s, BASE).ok()? {
            Target::Artifact { id, version } => Some((id, version)),
            Target::Page(_) => None,
        }
    }
    fn page(s: &str) -> Option<String> {
        match target(s, BASE).ok()? {
            Target::Page(u) => Some(u),
            Target::Artifact { .. } => None,
        }
    }

    #[test]
    fn ids_and_the_daemons_urls_are_artifacts() {
        assert_eq!(art("7q3k9mzx2b4t"), Some(("7q3k9mzx2b4t".into(), None)));
        assert_eq!(
            art("http://localhost:7480/a/7q3k9mzx2b4t/v/3"),
            Some(("7q3k9mzx2b4t".into(), Some(3)))
        );
        assert_eq!(
            art("http://127.0.0.1:7480/c/7q3k9mzx2b4t/v/2/x.html"),
            Some(("7q3k9mzx2b4t".into(), Some(2)))
        );
        assert_eq!(
            art("http://192.168.1.20:7480/a/7q3k9mzx2b4t"),
            Some(("7q3k9mzx2b4t".into(), None))
        );
        assert_eq!(
            art("http://7q3k9mzx2b4t.localhost:7480/v/1/"),
            Some(("7q3k9mzx2b4t".into(), Some(1)))
        );
        assert_eq!(
            art("localhost:7480/a/7q3k9mzx2b4t"),
            Some(("7q3k9mzx2b4t".into(), None))
        );
    }

    #[test]
    fn every_other_http_url_is_a_page() {
        assert_eq!(
            page("http://localhost:5173/"),
            Some("http://localhost:5173/".into())
        );
        assert_eq!(
            page("http://localhost:5173/a/7q3k9mzx2b4t"),
            Some("http://localhost:5173/a/7q3k9mzx2b4t".into()),
            "another port's /a/ path is a page"
        );
        assert_eq!(
            page("https://example.com/x?y#/z"),
            Some("https://example.com/x?y#/z".into())
        );
    }

    #[test]
    fn nonsense_is_invalid_id() {
        assert!(target("nope", BASE).is_err());
        assert!(target("ftp://localhost:7480/a/7q3k9mzx2b4t", BASE).is_err());
        assert!(
            target("http://localhost:7480/settings", BASE).is_err(),
            "the daemon's port names no page"
        );
    }

    /// The shared fixture's cases (a case's own `daemon_base` overrides the
    /// section's), also run by the Pi extension's `target()`.
    #[test]
    fn target_matches_the_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/pi/test/fixtures/contract.json");
        let f: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let base = f["target"]["daemon_base"].as_str().unwrap();
        let cases = f["target"]["cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        for c in cases {
            let input = c["input"].as_str().unwrap();
            let got = target(input, c["daemon_base"].as_str().unwrap_or(base));
            if c.get("error").is_some() {
                assert!(got.is_err(), "{input}");
            } else if let Some(p) = c["page"].as_str() {
                assert_eq!(got.ok(), Some(Target::Page(p.into())), "{input}");
            } else {
                assert_eq!(
                    got.ok(),
                    Some(Target::Artifact {
                        id: c["id"].as_str().unwrap().into(),
                        version: c["version"].as_u64().map(|v| v as u32),
                    }),
                    "{input}"
                );
            }
        }
    }
}

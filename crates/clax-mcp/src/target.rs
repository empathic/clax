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
/// http(s) URL on the daemon's port is an artifact reference when its host
/// is the daemon's: for a daemon on a local name ([`is_local`]), any host
/// (`localhost`, `127.0.0.1`, a LAN or tailnet address or name, or
/// `<id>.localhost` all reach it); for a daemon on any other name (a
/// remote or hosted Clax, often on https's default port), that name or a
/// subdomain of it. Any other
/// http(s) URL is a page. An artifact reference, and text without a
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
        let on_daemon = url::Url::parse(daemon_base).ok().is_some_and(|d| {
            d.port_or_known_default().is_some()
                && u.port_or_known_default() == d.port_or_known_default()
                && same_host(&u, &d)
        });
        if !on_daemon {
            return Ok(Target::Page(s.to_string()));
        }
    }
    artifact_ref(s)
        .map(|(id, version)| Target::Artifact { id, version })
        .map_err(|_| invalid(s))
}

/// Whether `url` is on the host of the daemon at `daemon`: any host when
/// the daemon's is local (the machine is reached by many names, LAN and
/// tailnet ones included, so its port alone tells), else the daemon's own
/// name or a subdomain of it.
fn same_host(url: &url::Url, daemon: &url::Url) -> bool {
    let (Some(h), Some(d)) = (url.host(), daemon.host()) else {
        return false;
    };
    if is_local(&d) {
        return true;
    }
    match (h, d) {
        (url::Host::Domain(h), url::Host::Domain(d)) => {
            let (h, d) = (h.trim_end_matches('.'), d.trim_end_matches('.'));
            h == d || h.strip_suffix(d).is_some_and(|sub| sub.ends_with('.'))
        }
        (h, d) => h == d,
    }
}

/// Whether `host` names this machine or its local network: `localhost`
/// and `*.localhost`, a `*.local` or single-label name, or a loopback,
/// private, link-local or unspecified address.
fn is_local(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(d) => {
            let d = d.trim_end_matches('.');
            d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || !d.contains('.')
        }
        url::Host::Ipv4(ip) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
        }
        url::Host::Ipv6(ip) => {
            let first = ip.segments()[0];
            ip.is_loopback()
                || ip.is_unspecified()
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80
        }
    }
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
    fn a_daemon_on_a_public_name_owns_only_its_own_host() {
        let t = |s: &str| target(s, "https://clax.example.com").ok();
        let page = |s: &str| Some(Target::Page(s.into()));
        let art = |v| {
            Some(Target::Artifact {
                id: "7q3k9mzx2b4t".into(),
                version: v,
            })
        };
        assert_eq!(
            t("https://staging.example.com/"),
            page("https://staging.example.com/")
        );
        assert_eq!(
            t("https://example.com/a/7q3k9mzx2b4t"),
            page("https://example.com/a/7q3k9mzx2b4t"),
            "another host's /a/ path is a page"
        );
        assert_eq!(
            t("https://localhost/a/7q3k9mzx2b4t"),
            page("https://localhost/a/7q3k9mzx2b4t")
        );
        assert_eq!(
            t("https://evilclax.example.com/a/7q3k9mzx2b4t"),
            page("https://evilclax.example.com/a/7q3k9mzx2b4t"),
            "a name that merely ends in the daemon's is another host"
        );
        assert_eq!(t("https://CLAX.example.com/a/7q3k9mzx2b4t/v/2"), art(Some(2)));
        assert_eq!(
            t("https://content.clax.example.com/c/7q3k9mzx2b4t/v/1/x.html"),
            art(Some(1))
        );
    }

    #[test]
    fn a_daemon_on_a_local_name_owns_every_host_on_its_port() {
        let t = |s: &str| target(s, "http://mac.local:7480").ok();
        let art = Some(Target::Artifact {
            id: "7q3k9mzx2b4t".into(),
            version: None,
        });
        assert_eq!(t("http://127.0.0.1:7480/a/7q3k9mzx2b4t"), art);
        assert_eq!(t("http://[::1]:7480/a/7q3k9mzx2b4t"), art);
        assert_eq!(t("http://10.0.0.5:7480/a/7q3k9mzx2b4t"), art);
        // A tailnet's addresses and names reach the daemon too.
        assert_eq!(t("http://100.101.102.103:7480/a/7q3k9mzx2b4t"), art);
        assert_eq!(t("http://mac.tail1234.ts.net:7480/a/7q3k9mzx2b4t"), art);
        assert_eq!(t("http://mac.home.arpa:7480/a/7q3k9mzx2b4t"), art);
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

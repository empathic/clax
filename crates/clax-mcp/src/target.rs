//! What a tool's `url_or_id` names (spec 2026-10-05 §6.2): an artifact (its
//! ID, or a URL of this daemon's), an artifact on another Clax, a Clax this
//! home is not connected to, or a page. [`target`] reads an ID and a URL of
//! the daemon's own; any other URL the daemon resolves (`GET
//! /api/resolve`), whose answer [`resolved`] reads.

use crate::render;
use crate::tools::artifact_ref;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

/// What a tool's `url_or_id` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An artifact, and the version when the URL names one.
    Artifact { id: String, version: Option<u32> },
    /// An artifact on a remote this home is connected to: the remote's ID,
    /// origin and name, the artifact's ID there, the version the URL names,
    /// its address there, and the local mirror of its threads while this
    /// home follows it (a session here watches it).
    Remote {
        remote: String,
        origin: String,
        name: String,
        artifact_id: String,
        version: Option<u32>,
        url: String,
        mirror: Option<String>,
    },
    /// A Clax instance this home is not connected to, by its origin and
    /// name, and the daemon's `request`: how to ask the person to connect
    /// it (`{tool, command}`; null from a daemon that gives none).
    SignInRequired {
        origin: String,
        name: String,
        request: Value,
    },
    /// A web page, by its URL as given.
    Page(String),
}

/// What the daemon's resolution `v` of `url` (`GET /api/resolve`) names.
/// An answer of a kind this client does not know, or one missing a field,
/// is the page `url`, as before the daemon resolved URLs.
pub fn resolved(v: &Value, url: &str) -> Target {
    let text = |k: &str| v[k].as_str().map(str::to_string);
    let version = v["version"].as_u64().and_then(|n| u32::try_from(n).ok());
    let page = || Target::Page(url.to_string());
    match v["kind"].as_str() {
        Some("local") => match text("artifact_id") {
            Some(id) if clax_core::ArtifactId::parse(&id).is_ok() => {
                Target::Artifact { id, version }
            }
            _ => page(),
        },
        Some("remote") => match (
            text("remote"),
            text("origin"),
            text("artifact_id"),
            text("url"),
        ) {
            (Some(remote), Some(origin), Some(artifact_id), Some(url)) => Target::Remote {
                name: text("name").unwrap_or_else(|| origin.clone()),
                mirror: text("mirror").filter(|m| clax_core::ArtifactId::parse(m).is_ok()),
                remote,
                origin,
                artifact_id,
                version,
                url,
            },
            _ => page(),
        },
        Some("sign_in_required") => match (text("origin"), text("name")) {
            (Some(origin), Some(name)) => Target::SignInRequired {
                origin,
                name,
                request: v["request"].clone(),
            },
            _ => page(),
        },
        _ => page(),
    }
}

/// The error for a URL on a Clax instance this home is not connected to:
/// `remote_sign_in_required`, naming it, the daemon's `request` (how to
/// ask the person to connect it: the tool and the CLI command), and the
/// tool that connects it.
pub fn sign_in_required(origin: &str, name: &str, request: &Value) -> CallToolResult {
    let tool = request["tool"].as_str().unwrap_or("connect_instance");
    render::error(
        "remote_sign_in_required",
        format!("{name} ({origin}) is a Clax instance this Clax is not connected to"),
        json!({
            "origin": origin,
            "name": name,
            "request": request,
            "next": format!(
                "Ask the person to connect it: call {tool} with {origin}, then call this tool again"
            ),
        }),
    )
}

/// The error for an artifact on connected remote `name` (`origin`),
/// which this tool does not change there: `not_on_remote`.
pub fn not_on_remote(name: &str, origin: &str, url: &str) -> CallToolResult {
    render::error(
        "not_on_remote",
        format!(
            "{url} is on {name} ({origin}), a connected Clax instance: this tool works on this Clax's artifacts, not on a remote artifact yet"
        ),
        json!({"origin": origin, "name": name, "url": url}),
    )
}

fn invalid(s: &str) -> CallToolResult {
    render::error(
        "invalid_id",
        format!("'{s}' is not an artifact ID, a Clax URL, or an http(s) page URL"),
        json!({}),
    )
}

/// What `url_or_id` names, for a daemon reached at `daemon_base`. An
/// http(s) URL on the daemon's scheme and port is an artifact reference
/// when its host is the daemon's: for a daemon on a local name ([`is_local`]), any host
/// (`localhost`, `127.0.0.1`, a LAN or tailnet address or name, or
/// `<id>.localhost` all reach it); for a daemon on any other name (a
/// remote or hosted Clax, often on https's default port), that name or a
/// subdomain of it. Any other
/// http(s) URL is a page, which the daemon may yet resolve to an artifact
/// of its own or of a remote (`GET /api/resolve`): a URL on the daemon's
/// port under another scheme is the daemon's to tell. An artifact
/// reference, and text without a
/// scheme, are read as [`artifact_ref`] reads them: an ID, or a Clax path
/// such as `localhost:7480/a/<id>`, `<id>.localhost:7480/_clax/v/<n>/…` or
/// `/c/<id>/v/<n>/…`.
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
                && u.scheme() == d.scheme()
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
            _ => None,
        }
    }
    fn page(s: &str) -> Option<String> {
        match target(s, BASE).ok()? {
            Target::Page(u) => Some(u),
            _ => None,
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
            art("http://7q3k9mzx2b4t.localhost:7480/_clax/v/1/"),
            Some(("7q3k9mzx2b4t".into(), Some(1)))
        );
        assert_eq!(
            art("http://7q3k9mzx2b4t.localhost:7480/about.html"),
            Some(("7q3k9mzx2b4t".into(), None))
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
        assert_eq!(
            t("https://CLAX.example.com/a/7q3k9mzx2b4t/v/2"),
            art(Some(2))
        );
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

    #[test]
    fn the_daemons_resolution_names_each_target() {
        const U: &str = "https://clax.example/a/7q3k9mzx2b4t/get?v=2#t";
        assert_eq!(
            resolved(
                &json!({"kind": "local", "artifact_id": "7q3k9mzx2b4t", "version": 3}),
                U
            ),
            Target::Artifact {
                id: "7q3k9mzx2b4t".into(),
                version: Some(3)
            }
        );
        assert_eq!(
            resolved(
                &json!({"kind": "remote", "remote": "r1", "origin": "https://clax.example",
                        "name": "acme", "artifact_id": "7q3k9mzx2b4t",
                        "url": "https://clax.example/a/7q3k9mzx2b4t"}),
                U
            ),
            Target::Remote {
                remote: "r1".into(),
                origin: "https://clax.example".into(),
                name: "acme".into(),
                artifact_id: "7q3k9mzx2b4t".into(),
                version: None,
                url: "https://clax.example/a/7q3k9mzx2b4t".into(),
                mirror: None,
            }
        );
        assert_eq!(
            resolved(
                &json!({"kind": "sign_in_required", "origin": "https://clax.example",
                        "name": "Acme"}),
                U
            ),
            Target::SignInRequired {
                origin: "https://clax.example".into(),
                name: "Acme".into(),
                request: Value::Null,
            }
        );
        let request = json!({"tool": "connect_instance",
            "command": "clax remote request https://clax.example/a/7q3k9mzx2b4t"});
        assert_eq!(
            resolved(
                &json!({"kind": "sign_in_required", "origin": "https://clax.example",
                        "name": "Acme", "request": request}),
                U
            ),
            Target::SignInRequired {
                origin: "https://clax.example".into(),
                name: "Acme".into(),
                request: request.clone(),
            }
        );
        assert_eq!(
            resolved(&json!({"kind": "page", "url": U}), U),
            Target::Page(U.into())
        );
        for odd in [
            json!({"kind": "followed", "url": U}),
            json!({"kind": "local", "artifact_id": "not an id"}),
            json!({"kind": "remote", "origin": "https://clax.example"}),
            json!({"kind": "sign_in_required", "origin": "https://clax.example"}),
            json!(null),
        ] {
            assert_eq!(resolved(&odd, U), Target::Page(U.into()), "{odd}");
        }
    }

    #[test]
    fn the_remote_errors_name_the_instance() {
        let body = |r: CallToolResult| -> Value {
            assert_eq!(r.is_error, Some(true));
            let t = r.content[0].as_text().unwrap().text.clone();
            serde_json::from_str(&t).unwrap()
        };
        let request = json!({"tool": "connect_instance",
            "command": "clax remote request https://clax.example/a/7q3k9mzx2b4t"});
        let v = body(sign_in_required("https://clax.example", "Acme", &request));
        assert_eq!(v["error"]["request"], request);
        assert_eq!(v["error"]["code"], "remote_sign_in_required");
        assert_eq!(v["error"]["origin"], "https://clax.example");
        assert_eq!(v["error"]["name"], "Acme");
        assert!(
            v["error"]["next"]
                .as_str()
                .unwrap()
                .contains("connect_instance"),
            "{v}"
        );
        let v = body(not_on_remote(
            "Acme",
            "https://clax.example",
            "https://clax.example/a/7q3k9mzx2b4t",
        ));
        assert_eq!(v["error"]["code"], "not_on_remote");
        assert_eq!(v["error"]["name"], "Acme");
        assert!(
            v["error"]["message"].as_str().unwrap().contains("Acme"),
            "{v}"
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

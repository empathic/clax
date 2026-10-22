//! Site hosts: `<id>.<site domain>[:port]` is artifact `<id>`'s own origin
//! (`<id>.localhost[:port]` on a local daemon). [`SiteHosts`] is the one
//! place a `Host` header is turned into an artifact; [`site_layer`] runs it
//! before every route.

use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use clax_core::ArtifactId;

/// The site domain of [`SiteHosts::local`].
pub const LOCAL_SITE_DOMAIN: &str = "localhost";

/// The host-to-artifact lookup, configured with the site domain
/// (`[hosted] site_domain`; `localhost` on a local daemon).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteHosts {
    domain: String,
}

/// What a site host names. Vanity names and custom hostnames will resolve
/// here too, in [`SiteHosts::target`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteTarget {
    pub artifact: ArtifactId,
}

/// Request extension: the request arrived on `artifact`'s site host, whose
/// origin is `origin` (`https://<id>.clax.run`, `http://<id>.localhost:7480`).
#[derive(Clone, Debug)]
pub struct SiteRequest {
    pub artifact: ArtifactId,
    pub origin: String,
}

/// `host` (a `Host` header) split at its port: `None` when a `:` is followed
/// by an empty or non-numeric port, or one past `u16`.
fn split_port(host: &str) -> Option<(&str, Option<u16>)> {
    match host.split_once(':') {
        None => Some((host, None)),
        Some((name, port)) => {
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((name, Some(port.parse().ok()?)))
        }
    }
}

impl SiteHosts {
    /// The local daemon's: sites at `<id>.localhost[:port]`.
    pub fn local() -> SiteHosts {
        SiteHosts {
            domain: LOCAL_SITE_DOMAIN.to_string(),
        }
    }

    /// Sites under `domain`, lowercased; `None` unless it is a bare host
    /// name ([`clax_core::config::site_domain`]): no scheme, port, path,
    /// leading dot or empty label.
    pub fn new(domain: &str) -> Option<SiteHosts> {
        Some(SiteHosts {
            domain: clax_core::config::site_domain(domain)?,
        })
    }

    /// The site domain (`clax.run`, `localhost`).
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// What `host` (a `Host` header) names: a `<12-char lowercase ID>` label
    /// directly under the site domain (the domain compared without regard
    /// to case), with an optional numeric port. Anything else, a nested
    /// name or the domain itself among them, is `None`.
    pub fn target(&self, host: &str) -> Option<SiteTarget> {
        let (name, _) = split_port(host)?;
        let cut = name.len().checked_sub(self.domain.len() + 1)?;
        let (label, rest) = name.split_at_checked(cut)?;
        let domain = rest.strip_prefix('.')?;
        if !domain.eq_ignore_ascii_case(&self.domain) || label.contains('.') {
            return None;
        }
        Some(SiteTarget {
            artifact: ArtifactId::parse(label).ok()?,
        })
    }

    /// `id`'s site origin on `scheme`, naming `port` unless it is the
    /// scheme's default (80 for `http`, 443 for `https`).
    pub fn origin(&self, id: &ArtifactId, scheme: &str, port: Option<u16>) -> String {
        let default = match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        match port.filter(|p| Some(*p) != default) {
            Some(p) => format!("{scheme}://{id}.{}:{p}", self.domain),
            None => format!("{scheme}://{id}.{}", self.domain),
        }
    }
}

/// Accepts `<12-char lowercase ID>.localhost` with an optional numeric
/// `:port`; anything else is `None` ([`SiteHosts::local`]). Its callers move
/// to [`AppState::sites`] with the site routes.
pub fn artifact_host(host: &str) -> Option<ArtifactId> {
    SiteHosts::local().target(host).map(|t| t.artifact)
}

/// The site request `host` (a `Host` header) makes under `sites`, or `None`
/// when it is not a site host: its origin is `https` when `secure` (the
/// public origin is), else `http`, with the `Host` header's port unless it
/// is the scheme's default.
fn site_request(sites: &SiteHosts, secure: bool, host: &str) -> Option<SiteRequest> {
    let target = sites.target(host)?;
    let port = split_port(host).and_then(|(_, p)| p);
    let scheme = if secure { "https" } else { "http" };
    Some(SiteRequest {
        origin: sites.origin(&target.artifact, scheme, port),
        artifact: target.artifact,
    })
}

/// Before every route: on a site host ([`AppState::sites`]) inserts
/// [`SiteRequest`], its origin on the public origin's scheme (`http` on a
/// local daemon), and leaves the path as it is: the router sends a site
/// request to the site routes alone. Every site response, an error's
/// included, carries the site's isolation headers
/// ([`crate::site_access::site_response_headers`]). Other hosts are
/// untouched.
pub async fn site_layer(State(s): State<AppState>, mut req: Request<Body>, next: Next) -> Response {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    let secure = s.hosted.as_ref().is_some_and(|h| h.secure);
    let Some(site) = site_request(s.sites(), secure, &host) else {
        return next.run(req).await;
    };
    req.extensions_mut().insert(site);
    let mut res = next.run(req).await;
    crate::site_access::site_response_headers(&s, &host, &mut res);
    res
}

#[cfg(test)]
mod tests {
    use super::{SiteHosts, artifact_host, site_request};

    #[test]
    fn a_site_request_takes_its_scheme_from_hosted_mode_and_its_port_from_the_host() {
        let sites = SiteHosts::new("clax.run").unwrap();
        let cases = [
            (
                true,
                "abcdefghjkmn.clax.run",
                "https://abcdefghjkmn.clax.run",
            ),
            (
                true,
                "abcdefghjkmn.clax.run:443",
                "https://abcdefghjkmn.clax.run",
            ),
            (
                true,
                "abcdefghjkmn.clax.run:8443",
                "https://abcdefghjkmn.clax.run:8443",
            ),
            (
                false,
                "abcdefghjkmn.clax.run:80",
                "http://abcdefghjkmn.clax.run",
            ),
            (
                false,
                "abcdefghjkmn.clax.run:7490",
                "http://abcdefghjkmn.clax.run:7490",
            ),
        ];
        for (secure, host, origin) in cases {
            let site = site_request(&sites, secure, host).unwrap_or_else(|| panic!("{host}"));
            assert_eq!(site.artifact.as_str(), "abcdefghjkmn", "{host}");
            assert_eq!(site.origin, origin, "{host}");
        }
        assert_eq!(
            site_request(&SiteHosts::local(), false, "abcdefghjkmn.localhost:7480")
                .unwrap()
                .origin,
            "http://abcdefghjkmn.localhost:7480"
        );
        assert!(site_request(&sites, true, "clax.run").is_none());
        assert!(site_request(&sites, true, "abcdefghjkmn.localhost").is_none());
    }

    #[test]
    fn target_accepts_an_id_under_the_domain_with_or_without_a_port() {
        let sites = SiteHosts::new("clax.run").unwrap();
        for host in ["abcdefghjkmn.clax.run", "abcdefghjkmn.clax.run:443"] {
            let t = sites.target(host).unwrap_or_else(|| panic!("{host}"));
            assert_eq!(t.artifact.as_str(), "abcdefghjkmn", "{host}");
        }
    }

    #[test]
    fn target_refuses_nested_uppercase_and_foreign_hosts() {
        let sites = SiteHosts::new("clax.run").unwrap();
        for bad in [
            "a.b.clax.run",
            "x.abcdefghjkmn.clax.run",
            "ABCDEFGHJKMN.clax.run",
            "abcdefghjkmn.clax.run.evil.com",
            "clax.run",
            ".clax.run",
            "abcdefghjkmn.clax.dev",
            "abcdefghjkmnclax.run",
            "abcdefghjkmn.clax.run:",
            "abcdefghjkmn.clax.run:x",
        ] {
            assert!(sites.target(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn local_hosts_keep_the_localhost_form() {
        let sites = SiteHosts::local();
        assert_eq!(sites.domain(), "localhost");
        assert_eq!(
            sites
                .target("abcdefghjkmn.localhost:7480")
                .unwrap()
                .artifact
                .as_str(),
            "abcdefghjkmn"
        );
        for bad in ["abcdefghjkmn.localhost:", "abcdefghjkmn.localhost:x"] {
            assert!(sites.target(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_site_domain_is_a_bare_host_name() {
        assert_eq!(SiteHosts::new("Clax.Run").unwrap().domain(), "clax.run");
        for bad in [
            "",
            " ",
            "https://clax.run",
            "clax.run:443",
            ".clax.run",
            "clax.run/",
            "clax..run",
        ] {
            assert!(SiteHosts::new(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn origin_names_the_scheme_and_any_port_but_the_default() {
        let sites = SiteHosts::new("clax.run").unwrap();
        let id = clax_core::ArtifactId::parse("abcdefghjkmn").unwrap();
        assert_eq!(
            sites.origin(&id, "https", None),
            "https://abcdefghjkmn.clax.run"
        );
        assert_eq!(
            sites.origin(&id, "https", Some(443)),
            "https://abcdefghjkmn.clax.run"
        );
        assert_eq!(
            SiteHosts::local().origin(&id, "http", Some(7480)),
            "http://abcdefghjkmn.localhost:7480"
        );
        assert_eq!(
            SiteHosts::local().origin(&id, "http", Some(80)),
            "http://abcdefghjkmn.localhost"
        );
    }

    #[test]
    fn accepts_only_anchored_lowercase_ids() {
        assert_eq!(
            artifact_host("7q3k9mzx2b4t.localhost").unwrap().as_str(),
            "7q3k9mzx2b4t"
        );
        assert_eq!(
            artifact_host("7q3k9mzx2b4t.localhost:7480")
                .unwrap()
                .as_str(),
            "7q3k9mzx2b4t"
        );
        for bad in [
            "localhost",
            "localhost:7480",
            "evil.localhost",
            "7Q3K9MZX2B4T.localhost",
            "7q3k9mzx2b4t.localhost.attacker.com",
            "x.7q3k9mzx2b4t.localhost",
            "7q3k9mzx2b4t.localhos",
            "7q3k9mzx2b4t.localhost:abc",
            "7q3k9mzx2b4i.localhost",
        ] {
            assert!(artifact_host(bad).is_none(), "{bad}");
        }
    }
}

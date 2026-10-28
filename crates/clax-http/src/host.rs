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
/// ([`crate::site_access::site_response_headers`]). A request there for a
/// service worker's script (`Service-Worker: script`) is answered, before
/// any route, with Clax's own script, which unregisters itself
/// ([`crate::content_policy::SERVICE_WORKER_KILL_SWITCH`]): no page
/// registers a worker of its own on a site host. Other hosts are
/// untouched.
pub async fn site_layer(State(s): State<AppState>, mut req: Request<Body>, next: Next) -> Response {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    if s.trusted_host_frames && crate::auth::trusted_host(&host, &s.trusted_hosts) {
        return trusted_frame_layer(s, host, req, next).await;
    }
    let secure = s.hosted.as_ref().is_some_and(|h| h.secure);
    let Some(mut site) = site_request(s.sites(), secure, &host) else {
        return next.run(req).await;
    };
    // In an app that embeds the core, the site's pages run on the app's
    // content origin, not on the `<id>.localhost` host the core asks with:
    // that origin is the one the site's checks compare an `Origin` with.
    if s.embed.as_ref().and_then(|e| e.site_hosts()).is_some() {
        site.origin = crate::site_access::site_origin(&s, &site.artifact, &host);
    }
    let mut res = if crate::content_policy::wants_service_worker(req.headers()) {
        use axum::response::IntoResponse;
        (
            axum::http::StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "text/javascript"),
                (axum::http::header::CACHE_CONTROL, "no-store"),
            ],
            crate::content_policy::SERVICE_WORKER_KILL_SWITCH,
        )
            .into_response()
    } else {
        req.extensions_mut().insert(site);
        next.run(req).await
    };
    crate::site_access::site_response_headers(&s, &host, &mut res);
    res
}

/// Where a shell on a `[serve] trusted_hosts` name frames artifact `<id>`
/// when `[serve] trusted_host_frames` is on: `/_clax/frame/<id>/…` of that
/// same host, `…` being the path the artifact's site would serve
/// (`/_clax/v/<n>/…`, `/_clax/api/…`, …).
pub const FRAME_PATH: &str = "/_clax/frame/";

/// The sandbox a trusted-host frame's documents get, by the shell's
/// `<iframe sandbox>` and by the `sandbox` directive of their
/// `Content-Security-Policy` alike (so one opened in a tab is sandboxed
/// too): no `allow-same-origin`, so each document's origin is opaque and
/// it shares no storage, cookie or DOM with the shell or another artifact
/// on the same host. As `FRAME_SANDBOX` in `web/shell/src/origin.ts`.
pub const FRAME_SANDBOX: &str = "allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals";

/// Request extension: a site request that came as a trusted-host frame's
/// (`/_clax/frame/<id>/…` on a `[serve] trusted_hosts` name with `[serve]
/// trusted_host_frames` on; [`trusted_frame_layer`]). The site's access
/// check takes its caller from the request's own identity, not a site
/// cookie (`crate::site_access::site_access_layer`), and its pages' bridge
/// tag names the trusted host as its shell and relays every runtime call
/// through it (`data-api="frame"`).
#[derive(Clone, Debug)]
pub struct TrustedFrame {
    /// The origins the shell on this host can have: `https://<Host>` and
    /// `http://<Host>` (a proxy such as `tailscale serve` terminates TLS,
    /// so the daemon cannot tell which the browser used).
    pub shell_origins: [String; 2],
}

/// `path` as `(<id>, <rest>)` when it is under [`FRAME_PATH`]: `<rest>` is
/// the path after the ID, `/` when there is none; `None` for a path that
/// is not, or names no well-formed artifact ID.
pub fn frame_target(path: &str) -> Option<(ArtifactId, String)> {
    let tail = path.strip_prefix(FRAME_PATH)?;
    let (id, rest) = match tail.find('/') {
        Some(i) => (&tail[..i], &tail[i..]),
        None => (tail, "/"),
    };
    Some((ArtifactId::parse(id).ok()?, rest.to_string()))
}

/// [`site_layer`] for a request on a `[serve] trusted_hosts` name with
/// `[serve] trusted_host_frames` on. A path under [`FRAME_PATH`] is a site
/// request ([`SiteRequest`]) of the artifact it names, with the path that
/// follows the ID, marked [`TrustedFrame`]; its origin, which the site API
/// compares a write's `Origin` with, is the shell's (`Origin` when it names
/// this host on `https` or `http`, else `https://<Host>`), as only the
/// shell's relay reaches the site API here. Its response carries the
/// site's isolation headers, the `sandbox` policy ([`FRAME_SANDBOX`]) and
/// `Cross-Origin-Resource-Policy: cross-origin` (the frame's opaque origin
/// is another origin than this host). The bridge's lazy parts
/// (`/_clax/bridge/…`), which the opaque frame imports, are answered with
/// `Access-Control-Allow-Origin: *`: they are the UI's own public files.
/// Anything else on the host is the public host's, untouched.
async fn trusted_frame_layer(
    s: AppState,
    host: String,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    use axum::http::{HeaderValue, header};
    let path = req.uri().path().to_string();
    let Some((artifact, rest)) = frame_target(&path) else {
        let parts = path.starts_with("/_clax/bridge/");
        let mut res = next.run(req).await;
        if parts {
            res.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
        }
        return res;
    };
    let shell_origins = [format!("https://{host}"), format!("http://{host}")];
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .filter(|o| shell_origins.iter().any(|s| s == o))
        .map_or_else(|| shell_origins[0].clone(), str::to_string);
    let pq = match req.uri().query() {
        Some(q) => format!("{rest}?{q}"),
        None => rest,
    };
    let Ok(uri) = pq.parse::<axum::http::Uri>() else {
        use axum::response::IntoResponse;
        return crate::error::ApiError::not_found().into_response();
    };
    *req.uri_mut() = uri;
    req.extensions_mut()
        .insert(SiteRequest { artifact, origin });
    req.extensions_mut().insert(TrustedFrame { shell_origins });
    let mut res = next.run(req).await;
    crate::site_access::site_response_headers(&s, &host, &mut res);
    let h = res.headers_mut();
    h.insert(
        crate::site_access::CORP,
        HeaderValue::from_static("cross-origin"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("sandbox {FRAME_SANDBOX}")) {
        h.append(header::CONTENT_SECURITY_POLICY, v);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::{SiteHosts, artifact_host, frame_target, site_request};

    #[test]
    fn a_frame_path_names_an_artifact_and_the_path_its_site_serves() {
        let (id, rest) = frame_target("/_clax/frame/abcdefghjkmn/_clax/v/3/").unwrap();
        assert_eq!(
            (id.as_str(), rest.as_str()),
            ("abcdefghjkmn", "/_clax/v/3/")
        );
        let (_, rest) = frame_target("/_clax/frame/abcdefghjkmn").unwrap();
        assert_eq!(rest, "/");
        for p in [
            "/_clax/frame/",
            "/_clax/frame/../api/token",
            "/_clax/frame/NOTANID/x",
            "/a/abcdefghjkmn",
            "/_clax/framex/abcdefghjkmn/",
        ] {
            assert!(frame_target(p).is_none(), "{p}");
        }
    }

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

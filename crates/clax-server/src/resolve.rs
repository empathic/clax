//! What a URL names, decided in one place (`GET /api/resolve`,
//! [`crate::routes::resolve`]): one of this daemon's artifacts, an artifact
//! on a Clax instance this home is connected to, a Clax instance it is not
//! connected to, or a web page ([`Resolved`]).
//!
//! The order ([`resolve`]):
//!
//! 1. The URL is one of this daemon's own artifact URLs: its base (any
//!    local name on its port, for a local daemon, as
//!    [`clax_mcp::target::target`] reads it) or its site domain. Read from
//!    the URL alone: no artifact is looked up. A URL on another scheme
//!    than the base's, or on the origin of a remote this home connected
//!    (on the same port, under another name), is not the daemon's.
//! 2. The URL is the public copy of an artifact this daemon pushed, found
//!    among the remote links by its URL or by its remote and public
//!    artifact ID: the local artifact. A followed link (a remote artifact
//!    a local agent watches, kept in a mirror here) is not one: its URL
//!    stays the remote's, naming its mirror.
//! 3. The URL is on a connected remote's origin, or under the site domain
//!    its discovery names, when no other connected remote's host or site
//!    domain overlaps that domain: that remote's artifact. On the origin of a
//!    remote that refused its token (`signed_out`): one to sign in to
//!    again.
//! 4. Its origin's `GET /.well-known/clax` says it is a Clax instance:
//!    one to sign in to.
//! 5. Otherwise, a page.
//!
//! Steps 2 to 4 are the owner's and the owner's agents' alone, and a local
//! daemon's (a hosted one has no remotes): any other caller, and every
//! caller of a hosted daemon, gets only a local answer or a page
//! ([`resolve_local`]).
//!
//! The URL forms read as artifacts are `<origin>/a/<id>[/…][?…][#…]`,
//! `<origin>/c/<id>/v/<n>/…` and `<id>.<site domain>` (with
//! `/_clax/v/<n>/…` naming a version); `/a/<id>/v/<n>` names version `n`.
//! A query or fragment names none.
//!
//! Discovery asks with no credential and follows no redirect
//! ([`crate::public::Client`]), only an `https` origin or an `http` one on
//! loopback ([`clax_core::config::hosted_origin`]). Its answers are kept in
//! [`DiscoveryCache`]: an answer naming a Clax, for a connected remote's
//! origin, until a call to it fails, and for any other origin for an hour
//! ([`clax_http::discovery_cache::POSITIVE_TTL_S`]); anything else for ten
//! minutes; each on the state's clock.

pub use clax_http::discovery_cache::DiscoveryCache;

use crate::public::Client;
use axum::http::StatusCode;
use clax_core::ArtifactId;
use clax_core::discovery::Discovery;
use clax_http::discovery_cache::{Cached, Instance};
use clax_http::error::ApiError;
use clax_http::host::SiteHosts;
use clax_http::state::AppState;
use clax_store::remotes::{Remote, RemoteLink};
use serde::Serialize;
use serde_json::Value;

/// What a URL names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resolved {
    /// One of this daemon's artifacts, and the version the URL names.
    Local {
        artifact_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<u32>,
    },
    /// An artifact on a connected remote: the remote's ID, origin and name
    /// here, the artifact's ID there and the version the URL names, and its
    /// address there (`<origin>/a/<artifact_id>`).
    Remote {
        remote: String,
        origin: String,
        name: String,
        artifact_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<u32>,
        url: String,
        /// The local mirror of its threads, while this home follows it
        /// (a local session watches it, or did within the week): the
        /// artifact `wait_for_feedback` waits on for it.
        #[serde(skip_serializing_if = "Option::is_none")]
        mirror: Option<String>,
    },
    /// A Clax instance this home is not connected to: the URL's origin,
    /// the name its discovery gives, and how an agent asks the person to
    /// connect it.
    SignInRequired {
        origin: String,
        name: String,
        request: SignInRequest,
    },
    /// Not a Clax artifact this daemon can name: a web page, the URL as
    /// given.
    Page { url: String },
}

/// How an agent asks the person to connect an instance: a connection
/// request (`POST /api/remotes/requests`), through the MCP tool `tool` or
/// the CLI's `command`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SignInRequest {
    pub tool: String,
    pub command: String,
}

impl SignInRequest {
    /// The request for `url`, a URL on the instance: the command names it
    /// as the URL parser wrote it (normalized and percent-encoded), quoted
    /// for a POSIX shell ([`shell_quote`]), so nothing in it runs.
    pub fn for_url(url: &url::Url) -> SignInRequest {
        SignInRequest {
            tool: "connect_instance".to_string(),
            command: format!("clax remote request {}", shell_quote(url.as_str())),
        }
    }
}

/// `s` as one word of a POSIX shell: single-quoted, each single quote in
/// it written `'\''`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// What `url` names, as the module comment orders it.
///
/// # Errors
/// 400 `invalid_url` for a URL that does not parse, is not `http(s)`, or
/// is on this daemon and names no artifact; the store's.
pub async fn resolve(s: &AppState, url: &str) -> Result<Resolved, ApiError> {
    let (raw, u) = parse(url)?;
    if s.hosted.is_some() {
        return Ok(own(s, &raw, &u, &[])?.unwrap_or(Resolved::Page { url: raw }));
    }
    let origin = u.origin().ascii_serialization();
    let (remotes, links) = s
        .store_call(|st| Ok((st.remotes()?, st.all_remote_links()?)))
        .await?;
    if let Some(own) = own(s, &raw, &u, &remotes)? {
        return Ok(own);
    }
    if let Some(l) = links.iter().find(|l| !l.followed && same_url(&l.url, &u)) {
        return Ok(local(l));
    }
    // A remote that refused its token is one to connect again.
    if let Some(r) = remotes
        .iter()
        .find(|r| r.status == "signed_out" && r.origin == origin)
    {
        return Ok(Resolved::SignInRequired {
            origin: r.origin.clone(),
            name: r.name.clone(),
            request: SignInRequest::for_url(&u),
        });
    }
    let remotes: Vec<Remote> = remotes
        .into_iter()
        .filter(|r| r.status == "connected")
        .collect();
    if let Some((r, at)) = on_remote(s, &remotes, &u, &origin).await {
        let Some((artifact_id, version)) = at else {
            return Ok(Resolved::Page { url: raw });
        };
        let link = |followed: bool| {
            links.iter().find(|l| {
                l.followed == followed && l.remote_id == r.id && l.remote_artifact_id == artifact_id
            })
        };
        if let Some(l) = link(false) {
            return Ok(local(l));
        }
        return Ok(Resolved::Remote {
            url: format!("{}/a/{artifact_id}", r.origin),
            remote: r.id.clone(),
            origin: r.origin.clone(),
            name: r.name.clone(),
            mirror: link(true).map(|l| l.artifact_id.clone()),
            artifact_id,
            version,
        });
    }
    Ok(match discover(s, &origin).await {
        Some(i) => Resolved::SignInRequired {
            origin,
            name: i.name,
            request: SignInRequest::for_url(&u),
        },
        None => Resolved::Page { url: raw },
    })
}

/// What `url` names to a caller that may not use remotes: one of this
/// daemon's artifacts (step 1), else a page.
///
/// # Errors
/// As [`resolve`]'s.
pub async fn resolve_local(s: &AppState, url: &str) -> Result<Resolved, ApiError> {
    let (raw, u) = parse(url)?;
    let remotes = match s.hosted {
        Some(_) => Vec::new(),
        None => s.store_call(|st| st.remotes()).await?,
    };
    Ok(own(s, &raw, &u, &remotes)?.unwrap_or(Resolved::Page { url: raw }))
}

fn invalid(message: String) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_url", message)
}

/// `url` trimmed, and parsed: an `http(s)` URL.
fn parse(url: &str) -> Result<(String, url::Url), ApiError> {
    let raw = url.trim().to_string();
    let u = url::Url::parse(&raw)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
        .ok_or_else(|| invalid(format!("'{raw}' is not an http(s) URL")))?;
    Ok((raw, u))
}

/// The local artifact that link `l` is the public copy of.
fn local(l: &RemoteLink) -> Resolved {
    Resolved::Local {
        artifact_id: l.artifact_id.clone(),
        version: None,
    }
}

/// `u`'s host with its port when it names one, as a `Host` header has it.
fn host_port(u: &url::Url) -> String {
    match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        _ => String::new(),
    }
}

/// The version a site URL's path names (`/_clax/v/<n>/…`).
fn site_version(u: &url::Url) -> Option<u32> {
    let mut segs = u.path_segments()?.filter(|s| !s.is_empty());
    match (segs.next(), segs.next(), segs.next()) {
        (Some("_clax"), Some("v"), Some(n)) => n.parse().ok().filter(|v: &u32| v.to_string() == n),
        _ => None,
    }
}

/// The artifact `u` names under site domain `domain`, served on `base`'s
/// scheme and port, and the version it names.
fn on_site(domain: &str, base: &url::Url, u: &url::Url) -> Option<(String, Option<u32>)> {
    if u.scheme() != base.scheme() || u.port_or_known_default() != base.port_or_known_default() {
        return None;
    }
    let t = SiteHosts::new(domain)?.target(&host_port(u))?;
    Some((t.artifact.as_str().to_string(), site_version(u)))
}

/// Step 1: `u` as one of this daemon's own artifact URLs, `None` when it is
/// another's: one on another scheme than the daemon's base, or on the
/// origin of one of `remotes` (connected or signed out), is another's,
/// whatever its port (a local daemon owns every host on its port).
fn own(
    s: &AppState,
    raw: &str,
    u: &url::Url,
    remotes: &[Remote],
) -> Result<Option<Resolved>, ApiError> {
    let base = match &s.hosted {
        Some(h) => h.origin.clone(),
        None => s.browser_base.clone(),
    };
    let origin = u.origin().ascii_serialization();
    let scheme_differs = url::Url::parse(&base).is_ok_and(|b| b.scheme() != u.scheme());
    if scheme_differs
        || remotes
            .iter()
            .any(|r| r.status != "pending" && r.origin == origin)
    {
        return Ok(None);
    }
    let local = |(artifact_id, version)| Resolved::Local {
        artifact_id,
        version,
    };
    match clax_mcp::target::target(raw, &base) {
        Ok(clax_mcp::target::Target::Artifact { id, version }) => {
            return Ok(Some(local((id, version))));
        }
        Ok(_) => {}
        Err(_) => {
            return Err(invalid(format!(
                "'{raw}' is on this Clax and names no artifact"
            )));
        }
    }
    let on_own_site = s.hosted.as_ref().and_then(|h| {
        let sites = h.sites.as_ref()?;
        on_site(sites.domain(), &url::Url::parse(&h.origin).ok()?, u)
    });
    Ok(on_own_site.map(local))
}

/// Whether `link` (`<origin>/a/<id>`) is the address `u` names: the same
/// origin and artifact, whatever follows.
fn same_url(link: &str, u: &url::Url) -> bool {
    let Ok(l) = url::Url::parse(link) else {
        return false;
    };
    l.origin() == u.origin()
        && matches!(
            (artifact_in_path(&l), artifact_in_path(u)),
            (Some((a, _)), Some((b, _))) if a == b
        )
}

/// The artifact and version `u`'s path names (`/a/<id>[/v/<n>]…`,
/// `/c/<id>/v/<n>/…`), as [`clax_mcp::tools::artifact_ref`] reads one.
fn artifact_in_path(u: &url::Url) -> Option<(String, Option<u32>)> {
    let path = format!("/{}", u.path().trim_start_matches('/'));
    clax_mcp::tools::artifact_ref(&path).ok()
}

/// Step 3: the connected remote `u` is on, by `origin` or the site domain
/// its discovery names ([`claimable`]), and the artifact it names there
/// (`None` for a page of the remote's that names none).
async fn on_remote<'r>(
    s: &AppState,
    remotes: &'r [Remote],
    u: &url::Url,
    origin: &str,
) -> Option<(&'r Remote, Option<(String, Option<u32>)>)> {
    if let Some(r) = remotes.iter().find(|r| r.origin == origin) {
        return Some((r, artifact_in_path(u)));
    }
    // Only a host whose first label is an artifact ID can be a site's.
    let first = u.host_str()?.split('.').next()?;
    ArtifactId::parse(first).ok()?;
    // Every remote's discovery at once: one that does not answer costs its
    // timeout once, not once per remote before it.
    let claims: Vec<(&Remote, String, Option<String>)> =
        futures::future::join_all(remotes.iter().map(|r| async move {
            let host = url::Url::parse(&r.origin)
                .ok()
                .and_then(|b| b.host_str().map(str::to_ascii_lowercase))
                .unwrap_or_default();
            let domain = discover_connected(s, &r.origin, true)
                .await
                .and_then(|i| i.site_domain);
            (r, host, domain)
        }))
        .await;
    for (i, (r, _, domain)) in claims.iter().enumerate() {
        let Some(domain) = domain else {
            continue;
        };
        let others: Vec<(&str, Option<&str>)> = claims
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, (_, h, d))| (h.as_str(), d.as_deref()))
            .collect();
        if !claimable(domain, &others) {
            continue;
        }
        let Ok(base) = url::Url::parse(&r.origin) else {
            continue;
        };
        if let Some(at) = on_site(domain, &base, u) {
            return Some((r, Some(at)));
        }
    }
    None
}

/// Whether `a` and `b` (lowercase host names) are the same name or one is
/// under the other, label by label.
fn overlap(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

/// Whether a connected remote may claim site domain `domain` (lowercase,
/// as [`SiteHosts`] keeps it), its sites' URLs then resolving to it: a
/// name of at least two labels, not `localhost` or under it (this
/// daemon's own sites), and overlapping (the same name, or one under the
/// other) neither the host nor the claimed site domain of any of the
/// `others` connected remotes, each `(origin host, site domain)`. Two
/// remotes claiming overlapping names both lose them, so neither takes
/// the other's sites. A site domain need not be under its remote's own
/// host: a public Clax serves its sites on a domain of their own.
fn claimable(domain: &str, others: &[(&str, Option<&str>)]) -> bool {
    domain.contains('.')
        && !overlap(domain, clax_http::host::LOCAL_SITE_DOMAIN)
        && others.iter().all(|(host, theirs)| {
            !overlap(domain, host) && theirs.is_none_or(|t| !overlap(domain, t))
        })
}

/// What `origin`'s discovery says, from [`AppState::discovery`] or asked
/// now: the Clax instance it is, or `None` when it is none, did not
/// answer, or may not be asked (not `https`, nor `http` on loopback). An
/// answer asked now is kept for an hour, `origin` being no connected
/// remote's.
pub async fn discover(s: &AppState, origin: &str) -> Option<Instance> {
    discover_connected(s, origin, false).await
}

/// [`discover`], an answer asked now kept until forgotten when `connected`
/// (`origin` a connected remote's).
async fn discover_connected(s: &AppState, origin: &str, connected: bool) -> Option<Instance> {
    clax_core::config::hosted_origin(origin)?;
    match s.discovery.get(origin) {
        Cached::Clax(i) => return Some(i),
        Cached::NotClax => return None,
        Cached::Unknown => {}
    }
    let found = Client::new(origin, None)
        .discovery()
        .await
        .ok()
        .and_then(|v| instance(v, origin));
    match &found {
        Some(i) if connected => s.discovery.put_clax(origin, i.clone()),
        Some(i) => s.discovery.put_clax_unconnected(origin, i.clone()),
        None => s.discovery.put_not_clax(origin),
    }
    found
}

/// The Clax instance discovery answer `v` describes
/// ([`Discovery::is_clax`]), named by its `name` or else `origin`'s host
/// and port.
fn instance(v: Value, origin: &str) -> Option<Instance> {
    let d: Discovery = serde_json::from_value(v).ok()?;
    if !d.is_clax() {
        return None;
    }
    let name = match d.name.trim() {
        "" => crate::public::default_name(origin),
        n => n.to_string(),
    };
    let site_domain = d
        .site_domain
        .as_deref()
        .and_then(SiteHosts::new)
        .map(|h| h.domain().to_string());
    Some(Instance {
        kind: d.kind,
        name,
        site_domain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The command's URL is one quoted word whatever the agent's URL held:
    /// an `&`, a `;`, a `$(…)`, a space or a single quote runs nothing.
    #[test]
    fn the_request_command_quotes_the_url_for_a_shell() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote(""), "''");
        let raw = "https://clax.example/a/7q3k9mzx2b4t?x=1&y=2;rm -rf $(whoami)#it's";
        let u = url::Url::parse(raw).unwrap();
        let cmd = SignInRequest::for_url(&u).command;
        let arg = cmd.strip_prefix("clax remote request ").unwrap();
        // One single-quoted word: quotes only at its ends, or as '\''.
        assert!(arg.starts_with('\'') && arg.ends_with('\''), "{cmd}");
        let inner = &arg[1..arg.len() - 1];
        assert!(!inner.replace("'\\''", "").contains('\''), "{cmd}");
        assert!(!inner.contains(' '), "a space is percent-encoded: {cmd}");
        // Unquoted as a shell would, it is the parsed URL again.
        assert_eq!(inner.replace("'\\''", "'"), u.as_str());
        // A literal single quote that reaches the word is escaped.
        let q = url::Url::parse("https://clax.example/a/x#a'b").unwrap();
        let cmd = SignInRequest::for_url(&q).command;
        assert_eq!(
            cmd,
            format!(
                "clax remote request '{}'",
                q.as_str().replace('\'', "'\\''")
            )
        );
    }

    #[test]
    fn resolved_serialises_as_its_kind() {
        let v = |r: Resolved| serde_json::to_value(r).unwrap();
        assert_eq!(
            v(Resolved::Local {
                artifact_id: "7q3k9mzx2b4t".into(),
                version: None
            }),
            json!({"kind": "local", "artifact_id": "7q3k9mzx2b4t"})
        );
        assert_eq!(
            v(Resolved::Remote {
                remote: "r1".into(),
                origin: "https://clax.example".into(),
                name: "acme".into(),
                artifact_id: "7q3k9mzx2b4t".into(),
                version: Some(2),
                url: "https://clax.example/a/7q3k9mzx2b4t".into(),
                mirror: None,
            }),
            json!({"kind": "remote", "remote": "r1", "origin": "https://clax.example",
                   "name": "acme", "artifact_id": "7q3k9mzx2b4t", "version": 2,
                   "url": "https://clax.example/a/7q3k9mzx2b4t"})
        );
        assert_eq!(
            v(Resolved::SignInRequired {
                origin: "https://clax.example".into(),
                name: "Acme".into(),
                request: SignInRequest::for_url(
                    &url::Url::parse("https://clax.example/a/7q3k9mzx2b4t").unwrap()
                ),
            }),
            json!({"kind": "sign_in_required", "origin": "https://clax.example", "name": "Acme",
                   "request": {"tool": "connect_instance",
                               "command": "clax remote request 'https://clax.example/a/7q3k9mzx2b4t'"}})
        );
        assert_eq!(
            v(Resolved::Page {
                url: "https://example.com/".into()
            }),
            json!({"kind": "page", "url": "https://example.com/"})
        );
    }

    #[test]
    fn only_a_clax_discovery_names_an_instance() {
        let o = "https://clax.example";
        let ok = json!({"kind": "hosted", "contract_version": "1", "api": "/api",
                        "name": "Acme", "site_domain": "sites.example"});
        assert_eq!(
            instance(ok, o),
            Some(Instance {
                kind: "hosted".into(),
                name: "Acme".into(),
                site_domain: Some("sites.example".into()),
            })
        );
        let unnamed = json!({"kind": "local", "contract_version": "1", "api": "/api"});
        assert_eq!(instance(unnamed, o).unwrap().name, "clax.example");
        for bad in [
            json!({"kind": "worker", "contract_version": "1", "api": "/api"}),
            json!({"kind": "hosted", "api": "/api"}),
            json!({"kind": "hosted", "contract_version": "1", "api": "/v2"}),
            json!("<html>"),
        ] {
            assert_eq!(instance(bad.clone(), o), None, "{bad}");
        }
        let odd_domain = json!({"kind": "hosted", "contract_version": "1", "api": "/api",
                                "site_domain": "https://x/"});
        assert_eq!(instance(odd_domain, o).unwrap().site_domain, None);
    }

    #[test]
    fn a_site_url_names_its_artifact_on_its_bases_scheme_and_port() {
        let base = url::Url::parse("https://clax.example").unwrap();
        let at = |s: &str| on_site("sites.example", &base, &url::Url::parse(s).unwrap());
        assert_eq!(
            at("https://7q3k9mzx2b4t.sites.example/_clax/v/3/x.html"),
            Some(("7q3k9mzx2b4t".into(), Some(3)))
        );
        assert_eq!(
            at("https://7q3k9mzx2b4t.SITES.example/about"),
            Some(("7q3k9mzx2b4t".into(), None))
        );
        assert_eq!(at("http://7q3k9mzx2b4t.sites.example/"), None);
        assert_eq!(at("https://7q3k9mzx2b4t.sites.example:8443/"), None);
        assert_eq!(at("https://a.7q3k9mzx2b4t.sites.example/"), None);
        assert_eq!(at("https://sites.example/"), None);
    }

    #[test]
    fn a_site_domain_is_claimed_only_when_no_other_remote_overlaps_it() {
        assert!(claimable("sites.example", &[]));
        assert!(claimable(
            "sites.example",
            &[("clax.other", Some("pages.other"))]
        ));
        // Its own host need not be its parent.
        assert!(claimable("clax.run", &[("clax.example", None)]));
        for bad in ["localhost", "a.localhost", "com", "example"] {
            assert!(!claimable(bad, &[]), "{bad}");
        }
        for (domain, others) in [
            // Another remote's origin host, or a name above or under it.
            ("clax.other", vec![("clax.other", None)]),
            ("other", vec![("clax.other", None)]),
            ("a.clax.other", vec![("clax.other", None)]),
            // Another remote's claimed site domain, or overlapping it.
            ("sites.example", vec![("b.example", Some("sites.example"))]),
            ("example", vec![("b.test", Some("sites.example"))]),
            ("x.sites.example", vec![("b.test", Some("sites.example"))]),
        ] {
            assert!(!claimable(domain, &others), "{domain} {others:?}");
        }
    }

    #[test]
    fn a_link_matches_its_artifact_whatever_follows() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        let link = "https://clax.example/a/7q3k9mzx2b4t";
        for same in [
            "https://clax.example/a/7q3k9mzx2b4t",
            "https://clax.example/a/7q3k9mzx2b4t/",
            "https://clax.example/a/7q3k9mzx2b4t/get?v=2#t",
            "https://clax.example/a/7q3k9mzx2b4t/v/3",
            "https://clax.example/c/7q3k9mzx2b4t/v/1/x.html",
        ] {
            assert!(same_url(link, &u(same)), "{same}");
        }
        for other in [
            "https://clax.example/a/8q3k9mzx2b4t",
            "https://other.example/a/7q3k9mzx2b4t",
            "http://clax.example/a/7q3k9mzx2b4t",
            "https://clax.example/",
        ] {
            assert!(!same_url(link, &u(other)), "{other}");
        }
    }
}

//! The apps home declares (`~/.config/clax/apps.json`, rendered by home's
//! `services.apps`; RECOMMEND-consultant-2026-11-02-clax-app-identities
//! Q1/Q2): each `app:<designator>` with its addresses, `(origin, prefix)`
//! pairs, one per network it is reached on.
//!
//! Clax reads it for what follows an app rather than a host:
//! - a queue's or an install's origins ([`Apps::expand`]): a listed
//!   `app:<designator>` stands for every origin of that app, and a listed
//!   origin that only one declared app is reached on stands for every
//!   origin of that app too, so a queue that lists `http://127.0.0.1:7700`
//!   admits Changeling's tailnet addresses the moment home declares them;
//! - Clax's own public origin ([`Apps::service_origin`] of [`SELF`]), which
//!   the install config and the reporter's view link name in place of the
//!   request's `Host`.
//!
//! A page resolves to an app by the longest prefix over the declared
//! addresses of its origin ([`Apps::resolve`]). A retired app is kept in
//! the file but follows nothing here. Fields Clax does not read are
//! ignored, so home may add columns.

use crate::{CoreError, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The designator of Clax's own app.
pub const SELF: &str = "clax";

/// The prefix of an app reference in an origin list: `app:<designator>`.
pub const APP_REF: &str = "app:";

#[derive(Clone, Debug, Default, Deserialize)]
struct RawAddress {
    origin: String,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    network: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RawApp {
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    addresses: Vec<RawAddress>,
    #[serde(default)]
    retired: Option<String>,
}

/// One declared address of an app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    /// `scheme://host[:port]`, as a browser's `Origin` names it.
    pub origin: String,
    /// `/`, or a path with no trailing `/` (`/fold`).
    pub prefix: String,
    /// `loopback` or `tailnet`, as declared (`None` when not).
    pub network: Option<String>,
}

/// One declared app.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct App {
    /// Its Tailscale Service name, when it has one.
    pub service: Option<String>,
    /// Its addresses, in the file's order.
    pub addresses: Vec<Address>,
    /// When it was retired, if it was.
    pub retired: Option<String>,
}

/// The declared apps, by designator. Empty when home declares none.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Apps {
    apps: BTreeMap<String, App>,
}

/// Whether `d` is a designator: `[a-z0-9][a-z0-9-]*`, at most 63 bytes.
pub fn is_designator(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 63
        && d.bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && d.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `raw` as an origin list names an app (`app:<designator>`), when it does.
pub fn app_ref(raw: &str) -> Option<&str> {
    raw.trim()
        .strip_prefix(APP_REF)
        .filter(|d| is_designator(d))
}

fn prefix_of(raw: Option<&str>) -> String {
    let p = raw.unwrap_or("/").trim_end_matches('/');
    if p.is_empty() {
        "/".to_string()
    } else if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    }
}

/// Whether `prefix` covers `path`: `/` covers every path, else the path is
/// the prefix or below it.
fn covers(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

impl Apps {
    /// The apps of an `apps.json` text.
    ///
    /// # Errors
    /// `bad_config` for text that is not an object of apps, a designator
    /// that is not one, or an address whose origin is not an origin.
    pub fn parse(text: &str) -> Result<Apps> {
        let raw: BTreeMap<String, RawApp> = serde_json::from_str(text)
            .map_err(|e| CoreError::invalid("bad_config", format!("apps.json: {e}")))?;
        let mut apps = BTreeMap::new();
        for (name, a) in raw {
            if !is_designator(&name) {
                return Err(CoreError::invalid(
                    "bad_config",
                    format!("apps.json: {name:?} is not an app designator"),
                ));
            }
            let mut addresses = Vec::with_capacity(a.addresses.len());
            for ad in a.addresses {
                let origin = crate::queue::normalize_origin(&ad.origin).map_err(|_| {
                    CoreError::invalid(
                        "bad_config",
                        format!("apps.json: app:{name}: {:?} is not an origin", ad.origin),
                    )
                })?;
                addresses.push(Address {
                    origin,
                    prefix: prefix_of(ad.prefix.as_deref()),
                    network: ad.network,
                });
            }
            apps.insert(
                name,
                App {
                    service: a.service.filter(|s| !s.is_empty()),
                    addresses,
                    retired: a.retired.filter(|r| !r.is_empty()),
                },
            );
        }
        Ok(Apps { apps })
    }

    /// The apps of the file at `path`; none when it does not exist.
    ///
    /// # Errors
    /// [`Apps::parse`]'s, and `bad_config` for a file that cannot be read.
    pub fn load(path: &Path) -> Result<Apps> {
        match std::fs::read_to_string(path) {
            Ok(text) => Apps::parse(&text).map_err(|e| match e {
                CoreError::Invalid { message, .. } => {
                    CoreError::invalid("bad_config", format!("{}: {message}", path.display()))
                }
                other => other,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Apps::default()),
            Err(e) => Err(CoreError::invalid(
                "bad_config",
                format!("{}: {e}", path.display()),
            )),
        }
    }

    /// Whether no app is declared.
    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }

    /// App `name`, retired or not.
    pub fn get(&self, name: &str) -> Option<&App> {
        self.apps.get(name)
    }

    fn live(&self) -> impl Iterator<Item = (&String, &App)> {
        self.apps.iter().filter(|(_, a)| a.retired.is_none())
    }

    /// The live app a page at `origin` and `path` is of: the one whose
    /// address at that origin has the longest prefix covering `path`.
    pub fn resolve(&self, origin: &str, path: &str) -> Option<&str> {
        let origin = crate::queue::normalize_origin(origin).ok()?;
        let path = if path.is_empty() { "/" } else { path };
        self.live()
            .flat_map(|(n, a)| a.addresses.iter().map(move |ad| (n, ad)))
            .filter(|(_, ad)| ad.origin == origin && covers(&ad.prefix, path))
            .max_by_key(|(_, ad)| if ad.prefix == "/" { 0 } else { ad.prefix.len() })
            .map(|(n, _)| n.as_str())
    }

    /// The distinct origins of live app `name`, in the file's order; none
    /// for an app not declared or retired.
    pub fn origins_of(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(a) = self.apps.get(name).filter(|a| a.retired.is_none()) {
            for ad in &a.addresses {
                if !out.contains(&ad.origin) {
                    out.push(ad.origin.clone());
                }
            }
        }
        out
    }

    /// The one live app reached on `origin`, when exactly one is: an
    /// origin two apps share (two paths of one host) names neither.
    pub fn sole_app_of(&self, origin: &str) -> Option<&str> {
        let mut found = self
            .live()
            .filter(|(_, a)| a.addresses.iter().any(|ad| ad.origin == origin))
            .map(|(n, _)| n.as_str());
        match (found.next(), found.next()) {
            (Some(n), None) => Some(n),
            _ => None,
        }
    }

    /// The origins an origin list `listed` admits: each origin listed; for
    /// each `app:<designator>`, that app's origins; and for each listed
    /// origin only one live app is reached on, that app's origins.
    /// Distinct, listed origins first. An `app:` entry of no declared app
    /// admits nothing.
    pub fn expand(&self, listed: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let push = |o: String, out: &mut Vec<String>| {
            if !out.contains(&o) {
                out.push(o);
            }
        };
        for l in listed {
            if app_ref(l).is_none() {
                push(l.clone(), &mut out);
            }
        }
        for l in listed {
            let app = match app_ref(l) {
                Some(d) => Some(d),
                None => self.sole_app_of(l),
            };
            if let Some(d) = app {
                for o in self.origins_of(d) {
                    push(o, &mut out);
                }
            }
        }
        out
    }

    /// Whether origin list `listed` admits `origin` ([`Apps::expand`]).
    pub fn admits(&self, listed: &[String], origin: &str) -> bool {
        self.expand(listed).iter().any(|o| o == origin)
    }

    /// The public origin of live app `name`: its Service's own origin
    /// (`https://<service>.<tailnet>`) when declared, else its first tailnet
    /// address at `/`, else none. A non-root prefix is no origin of the app.
    pub fn service_origin(&self, name: &str) -> Option<String> {
        let a = self.apps.get(name).filter(|a| a.retired.is_none())?;
        let tailnet = |ad: &&Address| ad.network.as_deref() == Some("tailnet") && ad.prefix == "/";
        if let Some(s) = &a.service {
            let host = format!("://{s}.");
            if let Some(ad) = a
                .addresses
                .iter()
                .filter(tailnet)
                .find(|ad| ad.origin.contains(&host))
            {
                return Some(ad.origin.clone());
            }
        }
        a.addresses.iter().find(tailnet).map(|ad| ad.origin.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape home renders (programs/apps.nix `appsJson`), after S3 for
    /// clax and with Changeling's bridge port declared.
    const FILE: &str = r#"{
      "clax": {"id": "app:clax", "label": "Clax", "declaredBy": "home", "service": "clax",
        "addresses": [
          {"origin": "http://127.0.0.1:7480", "prefix": "/", "network": "loopback"},
          {"origin": "https://phobos.tail556f5b.ts.net", "prefix": "/", "network": "tailnet"},
          {"origin": "https://clax.tail556f5b.ts.net", "prefix": "/", "network": "tailnet"}]},
      "basis": {"id": "app:basis", "service": "basis", "addresses": [
          {"origin": "http://127.0.0.1:7484", "prefix": "/", "network": "loopback"},
          {"origin": "https://phobos.tail556f5b.ts.net", "prefix": "/fold", "network": "tailnet"}]},
      "changeling": {"id": "app:changeling", "service": "changeling", "addresses": [
          {"origin": "http://127.0.0.1:7700", "prefix": "/", "network": "loopback"},
          {"origin": "https://phobos.tail556f5b.ts.net:8444", "prefix": "/", "network": "tailnet"},
          {"origin": "https://changeling.tail556f5b.ts.net", "prefix": "/", "network": "tailnet"}]},
      "old": {"id": "app:old", "retired": "2026-11-01", "addresses": [
          {"origin": "http://127.0.0.1:9999", "prefix": "/", "network": "loopback"}]}
    }"#;

    fn apps() -> Apps {
        Apps::parse(FILE).unwrap()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_page_is_of_the_app_with_the_longest_prefix() {
        let a = apps();
        let phobos = "https://phobos.tail556f5b.ts.net";
        assert_eq!(a.resolve(phobos, "/fold"), Some("basis"));
        assert_eq!(a.resolve(phobos, "/fold/x"), Some("basis"));
        assert_eq!(a.resolve(phobos, "/folder"), Some("clax"));
        assert_eq!(a.resolve(phobos, "/a/f4s6pxhbs8bg"), Some("clax"));
        assert_eq!(
            a.resolve("https://PHOBOS.tail556f5b.ts.net:443", "/"),
            Some("clax")
        );
        assert_eq!(
            a.resolve("http://127.0.0.1:7700", "/settings"),
            Some("changeling")
        );
        assert_eq!(a.resolve("http://127.0.0.1:5173", "/"), None);
        assert_eq!(a.resolve("http://127.0.0.1:9999", "/"), None, "retired");
    }

    #[test]
    fn a_queue_listing_an_apps_only_origin_follows_the_app() {
        let a = apps();
        // The Changeling queue as it is stored today: no hand edit.
        let listed = s(&["http://127.0.0.1:7700"]);
        assert_eq!(
            a.expand(&listed),
            s(&[
                "http://127.0.0.1:7700",
                "https://phobos.tail556f5b.ts.net:8444",
                "https://changeling.tail556f5b.ts.net",
            ])
        );
        assert!(a.admits(&listed, "https://phobos.tail556f5b.ts.net:8444"));
        assert!(!a.admits(&listed, "https://clax.tail556f5b.ts.net"));
    }

    #[test]
    fn a_shared_origin_follows_no_app_but_an_app_reference_does() {
        let a = apps();
        let phobos = s(&["https://phobos.tail556f5b.ts.net"]);
        assert_eq!(a.expand(&phobos), phobos, "clax and basis share it");
        let basis = s(&["app:basis"]);
        assert_eq!(
            a.expand(&basis),
            s(&["http://127.0.0.1:7484", "https://phobos.tail556f5b.ts.net"])
        );
        assert!(a.expand(&s(&["app:nope"])).is_empty());
        assert!(a.expand(&s(&["app:old"])).is_empty(), "retired");
        assert_eq!(
            Apps::default().expand(&s(&["http://127.0.0.1:7700", "app:changeling"])),
            s(&["http://127.0.0.1:7700"]),
            "nothing declared: the list as it is"
        );
    }

    #[test]
    fn the_service_origin_is_the_services_own_else_the_first_tailnet_root() {
        let a = apps();
        assert_eq!(
            a.service_origin("clax").as_deref(),
            Some("https://clax.tail556f5b.ts.net")
        );
        // Before S3: home renders no service address yet.
        let before = Apps::parse(
            r#"{"clax": {"service": "clax", "addresses": [
                {"origin": "http://127.0.0.1:7480", "prefix": "/", "network": "loopback"},
                {"origin": "https://phobos.tail556f5b.ts.net", "prefix": "/", "network": "tailnet"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            before.service_origin("clax").as_deref(),
            Some("https://phobos.tail556f5b.ts.net")
        );
        assert_eq!(a.service_origin("basis"), None, "only a path on phobos");
        assert_eq!(a.service_origin("old"), None);
        assert_eq!(Apps::default().service_origin(SELF), None);
    }

    #[test]
    fn a_bad_file_is_refused_and_a_missing_one_is_empty() {
        assert!(Apps::parse("[]").is_err());
        assert!(Apps::parse(r#"{"Bad Name": {}}"#).is_err());
        assert!(Apps::parse(r#"{"x": {"addresses": [{"origin": "nope"}]}}"#).is_err());
        assert!(Apps::parse(r#"{"x": {"addresses": [{"origin": "https://a.b/path"}]}}"#).is_err());
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Apps::load(&dir.path().join("apps.json"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn app_references_are_designators() {
        assert_eq!(app_ref("app:changeling"), Some("changeling"));
        assert_eq!(app_ref(" app:clax "), Some("clax"));
        assert_eq!(app_ref("app:"), None);
        assert_eq!(app_ref("app:-x"), None);
        assert_eq!(app_ref("app:Clax"), None);
        assert_eq!(app_ref("https://app.example.com"), None);
    }
}

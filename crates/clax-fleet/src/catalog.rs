//! The fleet's catalog (`fleet.toml`): the Clax instances it serves, each
//! with the `config.toml` its home is given, and, on the SQLite backend,
//! the shard that keeps it.
//!
//! ```toml
//! [[instance]]
//! name = "acme"
//! shard = 0        # SQLite: the StatefulSet ordinal whose volume keeps it
//! default = true   # answers a request whose Host names no instance
//! config = '''
//! [hosted]
//! public_origin = "https://acme.clax.example.com"
//! site_domain = "acme-sites.example.com"
//! '''
//! ```
//!
//! Every instance is a hosted Clax: its `config` has a `[hosted]` table
//! with a usable `public_origin` (`clax_core::config::hosted_origin`). A
//! request belongs to the instance whose public origin its `Host` names
//! (case aside, and with or without the scheme's default port), or whose
//! site domain it is a site host under (`<artifact ID>.<site_domain>`);
//! else to the default instance, when there is one. No two instances share
//! an origin or a site domain.

use clax_core::config::HomeConfig;
use clax_http::host::SiteHosts;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One instance of the catalog.
#[derive(Clone, Debug)]
pub struct Instance {
    /// Its name: lowercase letters, digits and inner dashes, at most 63.
    pub name: String,
    /// On the SQLite backend, the ordinal of the process whose volume keeps
    /// its home.
    pub shard: Option<u32>,
    /// Whether it answers requests whose `Host` names no instance.
    pub default: bool,
    /// The `config.toml` its home is given.
    pub config: String,
    /// Its public origin, normalized (`https://host[:port]`).
    pub origin: String,
    /// Its public origin's authority, lowercase, as a `Host` header names
    /// it without the scheme's default port.
    authority: String,
    sites: Option<SiteHosts>,
}

impl PartialEq for Instance {
    fn eq(&self, other: &Instance) -> bool {
        self.name == other.name
            && self.shard == other.shard
            && self.default == other.default
            && self.config == other.config
    }
}

/// The instances a fleet serves.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    pub instances: Vec<Instance>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    instance: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    shard: Option<u32>,
    #[serde(default)]
    default: bool,
    config: String,
}

/// Whether `name` is an instance name: 1 to 63 lowercase ASCII letters,
/// digits and dashes, neither first nor last a dash. It names the
/// instance's home directory and its rows in PostgreSQL.
pub fn valid_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `host` (a `Host` header) lowercased, without `:80` or `:443`.
fn normalize_host(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    for port in [":80", ":443"] {
        if let Some(bare) = h.strip_suffix(port) {
            return bare.to_string();
        }
    }
    h
}

impl Catalog {
    /// The catalog `text` describes, read as the file at `path`.
    ///
    /// # Errors
    /// A message naming `path` and what is wrong: TOML that does not parse,
    /// an unknown key, a bad name, a name, origin or site domain used twice,
    /// two defaults, an instance with no usable `[hosted]`, or (when
    /// `shards` is `Some`, the SQLite backend's count) an instance with no
    /// `shard` or one past the count.
    pub fn parse(path: &Path, text: &str, shards: Option<u32>) -> Result<Catalog, String> {
        let at = path.display();
        let file: File = toml::from_str(text).map_err(|e| format!("{at}: {e}"))?;
        let mut names = HashSet::new();
        let mut authorities = HashSet::new();
        let mut domains = HashSet::new();
        let mut defaults = 0;
        let mut instances = Vec::with_capacity(file.instance.len());
        for e in file.instance {
            let name = e.name;
            if !valid_name(&name) {
                return Err(format!(
                    "{at}: instance name {name:?} is not 1 to 63 lowercase letters, digits and inner dashes"
                ));
            }
            if !names.insert(name.clone()) {
                return Err(format!("{at}: instance {name} is named twice"));
            }
            match (shards, e.shard) {
                (Some(_), None) => {
                    return Err(format!(
                        "{at}: instance {name} has no shard; on the SQLite backend each instance names the ordinal whose volume keeps it"
                    ));
                }
                (Some(n), Some(s)) if s >= n => {
                    return Err(format!(
                        "{at}: instance {name}'s shard {s} is past the {n} shards"
                    ));
                }
                _ => {}
            }
            let config = HomeConfig::from_text(
                PathBuf::from(format!("{at} (instance {name})")),
                &e.config,
                std::iter::empty(),
            )
            .map_err(|err| format!("{at}: instance {name}'s config: {err}"))?;
            let hosted = config
                .hosted()
                .map_err(|err| format!("{at}: instance {name}'s [hosted]: {err}"))?
                .ok_or_else(|| {
                    format!(
                        "{at}: instance {name} has no [hosted] public_origin; a fleet serves hosted instances only"
                    )
                })?;
            let url = url::Url::parse(&hosted.public_origin)
                .map_err(|err| format!("{at}: instance {name}'s public_origin: {err}"))?;
            let authority = match (url.host_str(), url.port()) {
                (Some(h), Some(p)) => format!("{h}:{p}"),
                (Some(h), None) => h.to_string(),
                (None, _) => {
                    return Err(format!("{at}: instance {name}'s public_origin has no host"));
                }
            };
            let authority = normalize_host(&authority);
            if !authorities.insert(authority.clone()) {
                return Err(format!(
                    "{at}: instance {name}'s public origin {} is another instance's too",
                    hosted.public_origin
                ));
            }
            let sites = match &hosted.site_domain {
                Some(d) => {
                    let sites = SiteHosts::new(d).ok_or_else(|| {
                        format!("{at}: instance {name}'s site_domain {d:?} is not a host name")
                    })?;
                    if !domains.insert(sites.domain().to_string()) {
                        return Err(format!(
                            "{at}: instance {name}'s site domain {d} is another instance's too"
                        ));
                    }
                    Some(sites)
                }
                None => None,
            };
            if e.default {
                defaults += 1;
                if defaults > 1 {
                    return Err(format!("{at}: more than one instance is the default"));
                }
            }
            instances.push(Instance {
                name,
                shard: e.shard,
                default: e.default,
                config: e.config,
                origin: hosted.public_origin,
                authority,
                sites,
            });
        }
        Ok(Catalog { instances })
    }

    /// The catalog in the file at `path`, as [`Catalog::parse`] reads it.
    ///
    /// # Errors
    /// The file cannot be read, or [`Catalog::parse`]'s.
    pub fn load(path: &Path, shards: Option<u32>) -> Result<Catalog, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Catalog::parse(path, &text, shards)
    }

    /// The instance named `name`.
    pub fn get(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name == name)
    }

    /// The instance a request whose `Host` header is `host` belongs to: the
    /// one whose public origin it names, or whose site domain it is a site
    /// host under, else the default instance.
    pub fn resolve(&self, host: Option<&str>) -> Option<&Instance> {
        if let Some(host) = host {
            let h = normalize_host(host);
            if let Some(i) = self.instances.iter().find(|i| i.authority == h) {
                return Some(i);
            }
            if let Some(i) = self
                .instances
                .iter()
                .find(|i| i.sites.as_ref().is_some_and(|s| s.target(&h).is_some()))
            {
                return Some(i);
            }
        }
        self.instances.iter().find(|i| i.default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str, shards: Option<u32>) -> Result<Catalog, String> {
        Catalog::parse(Path::new("fleet.toml"), text, shards)
    }

    const TWO: &str = r#"
[[instance]]
name = "acme"
shard = 0
default = true
config = '''
[hosted]
public_origin = "https://acme.example.com"
site_domain = "acme-sites.example.com"
'''

[[instance]]
name = "globex"
shard = 1
config = '''
[hosted]
public_origin = "https://clax.globex.test:8443"
'''
"#;

    #[test]
    fn requests_find_their_instance_by_origin_or_site_host() {
        let c = parse(TWO, Some(2)).unwrap();
        let name = |h: Option<&str>| c.resolve(h).map(|i| i.name.as_str());
        assert_eq!(name(Some("ACME.example.com")), Some("acme"));
        assert_eq!(name(Some("acme.example.com:443")), Some("acme"));
        assert_eq!(name(Some("clax.globex.test:8443")), Some("globex"));
        // Another port is not the origin: the default answers.
        assert_eq!(name(Some("clax.globex.test")), Some("acme"));
        assert_eq!(
            name(Some("abcdefghjkmn.acme-sites.example.com")),
            Some("acme")
        );
        assert_eq!(name(None), Some("acme"));
        assert_eq!(c.get("globex").unwrap().shard, Some(1));
        assert_eq!(
            c.get("globex").unwrap().origin,
            "https://clax.globex.test:8443"
        );
    }

    #[test]
    fn without_a_default_an_unknown_host_has_no_instance() {
        let c = parse(&TWO.replace("default = true\n", ""), None).unwrap();
        assert!(c.resolve(Some("elsewhere.test")).is_none());
        assert!(c.resolve(None).is_none());
    }

    #[test]
    fn a_bad_catalog_is_refused_naming_what_is_wrong() {
        let cases = [
            (TWO.replace("\"globex\"", "\"acme\""), "named twice"),
            (TWO.replace("\"globex\"", "\"Globex\""), "lowercase"),
            (
                TWO.replace("https://clax.globex.test:8443", "https://acme.example.com"),
                "another instance's too",
            ),
            (TWO.replace("shard = 1\n", ""), "has no shard"),
            (TWO.replace("shard = 1", "shard = 2"), "past the 2 shards"),
            (
                TWO.replace(
                    "config = '''\n[hosted]\npublic_origin = \"https://clax.globex.test:8443\"",
                    "config = '''\n[serve]\nport = 1",
                ),
                "no [hosted]",
            ),
            (
                TWO.replace("name = \"globex\"", "name = \"globex\"\ndefault = true"),
                "more than one",
            ),
            (format!("{TWO}\nextra = 1\n"), "unknown field"),
        ];
        for (text, want) in cases {
            let err = parse(&text, Some(2)).unwrap_err();
            assert!(err.contains(want), "{want:?} not in {err:?}");
            assert!(err.starts_with("fleet.toml"), "{err}");
        }
        // The PostgreSQL backend needs no shard.
        assert!(parse(&TWO.replace("shard = 1\n", ""), None).is_ok());
    }

    #[test]
    fn instance_names_are_dns_labels_in_lowercase() {
        for ok in ["a", "acme", "acme-2", &"x".repeat(63)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-a", "a-", "A", "a_b", "a.b", &"x".repeat(64)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}

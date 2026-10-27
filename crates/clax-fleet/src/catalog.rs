//! The fleet's catalog (`fleet.toml`): the Clax instances it serves, each
//! with the `config.toml` its home is given, and, on the SQLite backend,
//! the number of shards and the shard that keeps each instance.
//!
//! ```toml
//! shards = 2       # SQLite: the StatefulSet's replicas
//!
//! [[instance]]
//! name = "acme"
//! shard = 0        # SQLite: the StatefulSet ordinal whose volume keeps it
//! create = true    # SQLite: may make its home (only for a new instance)
//! default = true   # answers a request whose Host names no instance
//! paused = false   # true: kept stopped, its hosts answered 503
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
//! an origin, and no instance's site domain is, is under or is above
//! another instance's site domain or public host: each tenant's pages stay
//! cross-site from every other tenant.
//!
//! A paused instance keeps its hosts: they answer 503 while it is moved or
//! restored, never the default instance in its place.

use clax_core::config::HomeConfig;
use clax_http::host::SiteHosts;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
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
    /// Kept stopped: its hosts answer 503.
    pub paused: bool,
    /// On the SQLite backend, whether a process may make its home where
    /// there is none (a new instance's first start); otherwise one with no
    /// home on its shard does not start (it was moved without its home).
    pub create: bool,
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
            && self.paused == other.paused
            && self.create == other.create
            && self.config == other.config
    }
}

/// The instances a fleet serves.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    /// On the SQLite backend, how many shards (processes) there are.
    pub shards: Option<u32>,
    pub instances: Vec<Instance>,
    by_authority: HashMap<String, usize>,
    by_site_domain: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
    default: Option<usize>,
}

impl PartialEq for Catalog {
    fn eq(&self, other: &Catalog) -> bool {
        self.shards == other.shards && self.instances == other.instances
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    shards: Option<u32>,
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
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    create: bool,
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

/// Whether host name `a` is `b` or under it, label by label.
fn at_or_under(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!(".{b}"))
}

impl Catalog {
    /// The catalog `text` describes, read as the file at `path`; `sqlite`
    /// asks for the SQLite backend's shards.
    ///
    /// # Errors
    /// A message naming `path` and what is wrong: TOML that does not parse,
    /// an unknown key, a bad name, a name or origin used twice, a site
    /// domain at, under or above another instance's site domain or public
    /// host, two defaults, an instance with no usable `[hosted]`, or, on
    /// SQLite, no `shards`, an instance with no `shard` or one past them.
    pub fn parse(path: &Path, text: &str, sqlite: bool) -> Result<Catalog, String> {
        let at = path.display();
        let file: File = toml::from_str(text).map_err(|e| format!("{at}: {e}"))?;
        let shards = match (sqlite, file.shards) {
            (true, None) => {
                return Err(format!(
                    "{at}: no shards; on the SQLite backend the catalog says how many shards (StatefulSet replicas) there are"
                ));
            }
            (true, Some(0)) => return Err(format!("{at}: shards is 0")),
            (_, s) => s,
        };
        let mut names = HashSet::new();
        let mut authorities = HashSet::new();
        // Each instance's public host (no port) and site domain, by name.
        let mut hosts: Vec<(String, String, Option<String>)> = Vec::new();
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
            match (shards.filter(|_| sqlite), e.shard) {
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
            let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
                return Err(format!("{at}: instance {name}'s public_origin has no host"));
            };
            let authority = match url.port() {
                Some(p) => format!("{host}:{p}"),
                None => host.clone(),
            };
            let authority = normalize_host(&authority);
            if !authorities.insert(authority.clone()) {
                return Err(format!(
                    "{at}: instance {name}'s public origin {} is another instance's too",
                    hosted.public_origin
                ));
            }
            let sites = match &hosted.site_domain {
                Some(d) => Some(SiteHosts::new(d).ok_or_else(|| {
                    format!("{at}: instance {name}'s site_domain {d:?} is not a host name")
                })?),
                None => None,
            };
            let domain = sites.as_ref().map(|s| s.domain().to_string());
            if let Some(d) = &domain {
                for (other, other_host, other_domain) in &hosts {
                    let clash = |b: &str| at_or_under(d, b) || at_or_under(b, d);
                    if clash(other_host) || other_domain.as_deref().is_some_and(clash) {
                        return Err(format!(
                            "{at}: instance {name}'s site domain {d} is, is under or is above a host of instance {other}: each tenant's sites must be cross-site from every other tenant's"
                        ));
                    }
                }
            }
            for (other, _, other_domain) in &hosts {
                if let Some(od) = other_domain
                    && (at_or_under(&host, od) || at_or_under(od, &host))
                {
                    return Err(format!(
                        "{at}: instance {name}'s public host {host} is, is under or is above instance {other}'s site domain {od}"
                    ));
                }
            }
            hosts.push((name.clone(), host, domain));
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
                paused: e.paused,
                create: e.create,
                config: e.config,
                origin: hosted.public_origin,
                authority,
                sites,
            });
        }
        let mut c = Catalog {
            shards,
            instances,
            ..Catalog::default()
        };
        for (i, inst) in c.instances.iter().enumerate() {
            c.by_name.insert(inst.name.clone(), i);
            c.by_authority.insert(inst.authority.clone(), i);
            if let Some(s) = &inst.sites {
                c.by_site_domain.insert(s.domain().to_string(), i);
            }
            if inst.default {
                c.default = Some(i);
            }
        }
        Ok(c)
    }

    /// The catalog in the file at `path`, as [`Catalog::parse`] reads it.
    ///
    /// # Errors
    /// The file cannot be read, or [`Catalog::parse`]'s.
    pub fn load(path: &Path, sqlite: bool) -> Result<Catalog, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Catalog::parse(path, &text, sqlite)
    }

    /// The instance named `name`.
    pub fn get(&self, name: &str) -> Option<&Instance> {
        self.by_name.get(name).map(|i| &self.instances[*i])
    }

    /// The instance a request whose `Host` header is `host` belongs to: the
    /// one whose public origin it names, or whose site domain it is a site
    /// host under, else the default instance.
    pub fn resolve(&self, host: Option<&str>) -> Option<&Instance> {
        if let Some(host) = host {
            let h = normalize_host(host);
            if let Some(i) = self.by_authority.get(&h) {
                return Some(&self.instances[*i]);
            }
            // A site host is one label above its domain, with any port.
            let name = h.rsplit_once(':').map_or(h.as_str(), |(n, p)| {
                if p.bytes().all(|b| b.is_ascii_digit()) {
                    n
                } else {
                    h.as_str()
                }
            });
            if let Some((_, domain)) = name.split_once('.')
                && let Some(i) = self.by_site_domain.get(domain)
                && self.instances[*i]
                    .sites
                    .as_ref()
                    .is_some_and(|s| s.target(&h).is_some())
            {
                return Some(&self.instances[*i]);
            }
        }
        self.default.map(|i| &self.instances[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str, sqlite: bool) -> Result<Catalog, String> {
        Catalog::parse(Path::new("fleet.toml"), text, sqlite)
    }

    const TWO: &str = r#"
shards = 2

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
        let c = parse(TWO, true).unwrap();
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
        assert_eq!(
            name(Some("abcdefghjkmn.acme-sites.example.com:8443")),
            Some("acme")
        );
        // Not a site host: two labels, or not an artifact ID.
        assert_eq!(name(Some("a.b.acme-sites.example.com")), Some("acme"));
        assert_eq!(name(None), Some("acme"));
        assert_eq!(c.get("globex").unwrap().shard, Some(1));
        assert_eq!(c.shards, Some(2));
        assert_eq!(
            c.get("globex").unwrap().origin,
            "https://clax.globex.test:8443"
        );
    }

    #[test]
    fn without_a_default_an_unknown_host_has_no_instance() {
        let c = parse(&TWO.replace("default = true\n", ""), false).unwrap();
        assert!(c.resolve(Some("elsewhere.test")).is_none());
        assert!(c.resolve(Some("abcdefghjkmn.other.test")).is_none());
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
            (TWO.replace("shards = 2\n", ""), "no shards"),
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
            let err = parse(&text, true).unwrap_err();
            assert!(err.contains(want), "{want:?} not in {err:?}");
            assert!(err.starts_with("fleet.toml"), "{err}");
        }
        // The PostgreSQL backend needs no shards.
        let pg = TWO.replace("shard = 1\n", "").replace("shards = 2\n", "");
        assert!(parse(&pg, false).is_ok());
    }

    /// No tenant's site domain may be at, under or above another tenant's
    /// public host or site domain, either way round.
    #[test]
    fn site_domains_stay_cross_site_from_other_tenants() {
        let two = |a_origin: &str, a_sites: &str, b_origin: &str, b_sites: Option<&str>| {
            let b_sites = b_sites
                .map(|d| format!("site_domain = \"{d}\"\n"))
                .unwrap_or_default();
            format!(
                "[[instance]]\nname = \"a\"\nconfig = '''\n[hosted]\npublic_origin = \"{a_origin}\"\nsite_domain = \"{a_sites}\"\n'''\n\n[[instance]]\nname = \"b\"\nconfig = '''\n[hosted]\npublic_origin = \"{b_origin}\"\n{b_sites}'''\n"
            )
        };
        let refused = [
            two(
                "https://a.test",
                "example.com",
                "https://b.example.com",
                None,
            ),
            two(
                "https://a.test",
                "x.example.com",
                "https://example.com",
                None,
            ),
            two(
                "https://a.test",
                "sites.example.com",
                "https://b.test",
                Some("x.sites.example.com"),
            ),
            two(
                "https://a.test",
                "x.sites.example.com",
                "https://b.test",
                Some("sites.example.com"),
            ),
        ];
        for text in refused {
            let err = parse(&text, false).unwrap_err();
            assert!(
                err.contains("cross-site") || err.contains("site domain"),
                "{err}"
            );
        }
        assert!(
            parse(
                &two(
                    "https://a.test",
                    "a-sites.test",
                    "https://b.test",
                    Some("b-sites.test")
                ),
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn paused_and_create_are_read() {
        let c = parse(
            &TWO.replace("shard = 0\n", "shard = 0\npaused = true\ncreate = true\n"),
            true,
        )
        .unwrap();
        let acme = c.get("acme").unwrap();
        assert!(acme.paused && acme.create);
        assert!(!c.get("globex").unwrap().paused);
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

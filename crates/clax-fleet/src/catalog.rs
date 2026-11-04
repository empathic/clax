//! The fleet's catalog (`fleet.toml`): the Clax instances it serves, each
//! with the `config.toml` its home is given, the number of shards and the
//! shard that keeps each instance.
//!
//! ```toml
//! shards = 2       # the StatefulSet's replicas
//!
//! [[instance]]
//! name = "acme"
//! shard = 0        # the StatefulSet ordinal whose volume keeps it
//! create = true    # may make its home (only for a new instance)
//! default = false  # true: answers a request whose Host names no instance (one tenant only)
//! paused = false   # true: kept stopped, its hosts answered 503
//! config = '''
//! [hosted]
//! public_origin = "https://acme.clax.example.com"
//! site_domain = "acme-sites.example.net"
//! setup_secret_env = "CLAX_SETUP_SECRET_ACME"
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
//! another instance's site domain or public host, so no tenant's site host
//! is another tenant's host. That is not what makes them separate *sites*
//! to a browser, which goes by registrable domain (the Public Suffix List):
//! two tenants' hosts under one registrable domain are same-site, whatever
//! the nesting, so the operator gives each tenant's site domain a
//! registrable domain of its own (or one on the list), as the example's is
//! (`docs/kubernetes.md`, The catalog).
//!
//! Each instance reads its setup secret from the variable its
//! `setup_secret_env` names, and its GitHub client secret (when it signs in
//! with GitHub) from its `github_client_secret_env`'s, in the one
//! environment every instance of a process shares: no variable is read by
//! two instances (nor left by both to the same default) or for two
//! secrets, and none is a variable of the fleet's own.
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
    pub shard: u32,
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
    /// How many shards (processes) there are.
    pub shards: u32,
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
/// instance's home directory.
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

/// The fleet's own variables, which no instance's `setup_secret_env` may
/// name: its setup secret would be the fleet's key or database URL.
const FLEET_VARIABLES: [&str; 2] = ["CLAX_KEK", "CLAX_FLEET_PEER_KEY"];

/// A variable an instance reads a secret from, which no other may read
/// ([`Catalog::parse`]).
struct SecretRead {
    /// The `[hosted]` setting that names it.
    setting: &'static str,
    /// The secret it holds.
    secret: &'static str,
    /// What sharing it would do, after a comma.
    why: &'static str,
    /// A name for it of the instance's own.
    suggest: &'static str,
    env: String,
}

/// Whether host name `a` is `b` or under it, label by label.
fn at_or_under(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!(".{b}"))
}

impl Catalog {
    /// The catalog `text` describes, read as the file at `path`.
    ///
    /// # Errors
    /// A message naming `path` and what is wrong: TOML that does not parse,
    /// an unknown key, a bad name, a name or origin used twice, a site
    /// domain at, under or above another instance's site domain or public
    /// host, two instances reading one setup secret variable or one reading
    /// the fleet's own, two defaults, an instance with no usable `[hosted]`,
    /// no `shards`, or an instance with no `shard` or one past them.
    pub fn parse(path: &Path, text: &str) -> Result<Catalog, String> {
        let at = path.display();
        let file: File = toml::from_str(text).map_err(|e| format!("{at}: {e}"))?;
        let shards = match file.shards {
            None => {
                return Err(format!(
                    "{at}: no shards; the catalog says how many shards (StatefulSet replicas) there are"
                ));
            }
            Some(0) => return Err(format!("{at}: shards is 0")),
            Some(n) => n,
        };
        let mut names = HashSet::new();
        let mut authorities = HashSet::new();
        // Each secret variable, by the instance that reads it and the
        // secret it holds there.
        let mut secrets: HashMap<String, (String, &str)> = HashMap::new();
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
            let shard = match e.shard {
                None => {
                    return Err(format!(
                        "{at}: instance {name} has no shard; each instance names the ordinal whose volume keeps it"
                    ));
                }
                Some(s) if s >= shards => {
                    return Err(format!(
                        "{at}: instance {name}'s shard {s} is past the {shards} shards"
                    ));
                }
                Some(s) => s,
            };
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
            // Every variable the instance reads a secret from: its setting,
            // the secret, why it may be no other's, and a name to give it.
            let mut reads = vec![SecretRead {
                setting: "setup_secret_env",
                secret: "setup secret",
                why: ", so either's would open the other's /setup",
                suggest: "CLAX_SETUP_SECRET_<NAME>",
                env: hosted.setup_secret_env.clone(),
            }];
            if let Some(github) = &hosted.github {
                reads.push(SecretRead {
                    setting: "github_client_secret_env",
                    secret: "GitHub client secret",
                    why: ", though each instance's GitHub OAuth app has a secret of its own",
                    suggest: "CLAX_GITHUB_SECRET_<NAME>",
                    env: github.client_secret_env.clone(),
                });
            }
            for SecretRead {
                setting,
                secret,
                why,
                suggest,
                env,
            } in reads
            {
                if FLEET_VARIABLES.contains(&env.as_str()) {
                    return Err(format!(
                        "{at}: instance {name}'s {setting} is {env}, a variable of the fleet's own; name one for its {secret} alone"
                    ));
                }
                match secrets.get(&env) {
                    Some((other, theirs)) if *theirs == secret => {
                        return Err(format!(
                            "{at}: instances {other} and {name} both read their {secret} from {env}{why}; give each a {setting} of its own ({suggest})"
                        ));
                    }
                    Some((other, theirs)) => {
                        return Err(format!(
                            "{at}: instance {name}'s {secret} is read from {env}, which is instance {other}'s {theirs}; give each secret a variable of its own"
                        ));
                    }
                    None => {
                        secrets.insert(env, (name.clone(), secret));
                    }
                }
            }
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
                            "{at}: instance {name}'s site domain {d} is, is under or is above a host of instance {other}: no tenant's sites may be at another tenant's hosts"
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
                shard,
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
    pub fn load(path: &Path) -> Result<Catalog, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Catalog::parse(path, &text)
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
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Catalog, String> {
        Catalog::parse(Path::new("fleet.toml"), text)
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
setup_secret_env = "CLAX_SETUP_SECRET_ACME"
'''

[[instance]]
name = "globex"
shard = 1
config = '''
[hosted]
public_origin = "https://clax.globex.test:8443"
setup_secret_env = "CLAX_SETUP_SECRET_GLOBEX"
'''
"#;

    /// `text` with instance `name` signing in with GitHub, its client
    /// secret in variable `env` (the default variable when `None`).
    fn with_github(text: &str, name: &str, env: Option<&str>) -> String {
        let setup = format!(
            "setup_secret_env = \"CLAX_SETUP_SECRET_{}\"\n",
            name.to_uppercase()
        );
        let env = env
            .map(|e| format!("github_client_secret_env = \"{e}\"\n"))
            .unwrap_or_default();
        assert!(text.contains(&setup), "{name} is not in the catalog");
        text.replace(
            &setup,
            &format!("{setup}github_client_id = \"Iv1.{name}\"\n{env}"),
        )
    }

    /// Instances may each sign in with GitHub, each with a secret of its
    /// own; one alone may leave it to the default variable.
    #[test]
    fn each_instance_reads_its_github_secret_from_a_variable_of_its_own() {
        parse(&with_github(TWO, "acme", None)).unwrap();
        let both = with_github(
            &with_github(TWO, "acme", Some("CLAX_GITHUB_SECRET_ACME")),
            "globex",
            Some("CLAX_GITHUB_SECRET_GLOBEX"),
        );
        parse(&both).unwrap();
    }

    #[test]
    fn requests_find_their_instance_by_origin_or_site_host() {
        let c = parse(TWO).unwrap();
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
        assert_eq!(c.get("globex").unwrap().shard, 1);
        assert_eq!(c.shards, 2);
        assert_eq!(
            c.get("globex").unwrap().origin,
            "https://clax.globex.test:8443"
        );
    }

    #[test]
    fn without_a_default_an_unknown_host_has_no_instance() {
        let c = parse(&TWO.replace("default = true\n", "")).unwrap();
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
                    "config = '''\n[hosted]\npublic_origin = \"https://clax.globex.test:8443\"\nsetup_secret_env = \"CLAX_SETUP_SECRET_GLOBEX\"",
                    "config = '''\n[serve]\nport = 1",
                ),
                "no [hosted]",
            ),
            (
                TWO.replace("name = \"globex\"", "name = \"globex\"\ndefault = true"),
                "more than one",
            ),
            (format!("{TWO}\nextra = 1\n"), "unknown field"),
            (
                TWO.replace("CLAX_SETUP_SECRET_GLOBEX", "CLAX_SETUP_SECRET_ACME"),
                "instances acme and globex both read their setup secret from CLAX_SETUP_SECRET_ACME",
            ),
            // Both left to the default variable.
            (
                TWO.replace("setup_secret_env = \"CLAX_SETUP_SECRET_ACME\"\n", "")
                    .replace("setup_secret_env = \"CLAX_SETUP_SECRET_GLOBEX\"\n", ""),
                "both read their setup secret from CLAX_SETUP_SECRET",
            ),
            (
                TWO.replace("CLAX_SETUP_SECRET_GLOBEX", "CLAX_FLEET_PEER_KEY"),
                "a variable of the fleet's own",
            ),
            (
                TWO.replace("CLAX_SETUP_SECRET_GLOBEX", "CLAX_KEK"),
                "a variable of the fleet's own",
            ),
            // Both sign in with GitHub, their secrets left to the default
            // variable.
            (
                with_github(&with_github(TWO, "acme", None), "globex", None),
                "instances acme and globex both read their GitHub client secret from GITHUB_CLIENT_SECRET",
            ),
            (
                with_github(TWO, "acme", Some("CLAX_SETUP_SECRET_GLOBEX")),
                "instance globex's setup secret is read from CLAX_SETUP_SECRET_GLOBEX, which is instance acme's GitHub client secret",
            ),
            (
                with_github(TWO, "globex", Some("CLAX_FLEET_PEER_KEY")),
                "instance globex's github_client_secret_env is CLAX_FLEET_PEER_KEY, a variable of the fleet's own",
            ),
        ];
        for (text, want) in cases {
            let err = parse(&text).unwrap_err();
            assert!(err.contains(want), "{want:?} not in {err:?}");
            assert!(err.starts_with("fleet.toml"), "{err}");
        }
    }

    /// No tenant's site domain may be at, under or above another tenant's
    /// public host or site domain, either way round.
    #[test]
    fn no_site_domain_is_at_another_tenants_hosts() {
        let two = |a_origin: &str, a_sites: &str, b_origin: &str, b_sites: Option<&str>| {
            let b_sites = b_sites
                .map(|d| format!("site_domain = \"{d}\"\n"))
                .unwrap_or_default();
            format!(
                "shards = 1\n\n[[instance]]\nname = \"a\"\nshard = 0\nconfig = '''\n[hosted]\npublic_origin = \"{a_origin}\"\nsite_domain = \"{a_sites}\"\nsetup_secret_env = \"CLAX_SETUP_SECRET_A\"\n'''\n\n[[instance]]\nname = \"b\"\nshard = 0\nconfig = '''\n[hosted]\npublic_origin = \"{b_origin}\"\nsetup_secret_env = \"CLAX_SETUP_SECRET_B\"\n{b_sites}'''\n"
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
            let err = parse(&text).unwrap_err();
            assert!(err.contains("site domain"), "{err}");
        }
        assert!(
            parse(&two(
                "https://a.test",
                "a-sites.test",
                "https://b.test",
                Some("b-sites.test")
            ),)
            .is_ok()
        );
    }

    #[test]
    fn paused_and_create_are_read() {
        let c = parse(&TWO.replace("shard = 0\n", "shard = 0\npaused = true\ncreate = true\n"))
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

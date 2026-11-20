//! A browser session's capabilities as macaroons (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md` §18).
//!
//! # The identifier and the key
//!
//! A capability is a [`Macaroon`] whose identifier is
//! `v1/<key ID>/<session>/<chain>/<run>` (§18.1): the instance key's ID,
//! the session's ID, the chain of capability IDs from the session's root to
//! this capability (`.`-joined, each 22 base64url characters of 16 random
//! bytes), and the run that opened the session. It is signed with the
//! session's root key, derived from the instance key ([`Keys`], §18.2), and
//! serialized with the prefix [`PREFIX`].
//!
//! # Caveats
//!
//! Each caveat is `name=value` ([`Caveat`], §18.3): `actions` (the
//! `clax-browser` session actions a call may be of, [`Class`]), `expires` (Unix milliseconds on the
//! registry's clock), `holder` (`run:<ID>` or `grant:<ID>`) and `origin`
//! (the origins a `navigate` call may reach). [`Cap::check`] holds every
//! caveat against one call ([`Use`]); a caveat it does not understand
//! refuses every call, so a capability is never read as wider than its
//! caveats. A child ([`Keys::child`]) has its parent's caveats and more,
//! and its parent's chain and one more ID, so revoking an ID revokes
//! every capability under it (§18.6).

use clax_core::macaroon::{Macaroon, derive_key};
use clax_core::secret::{base64url, random_bytes};
use std::collections::BTreeSet;

/// The prefix of a serialized browser capability.
pub const PREFIX: &str = "cxk_";
/// The identifier's version.
const VERSION: &str = "v1";
/// The purpose of the instance key capabilities are made with
/// (`clax_store::secrets::Kek::instance_key`, §18.2).
pub const KEY_PURPOSE: &str = "browser-capabilities";

/// What a call does, as caveats name it (§18.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Read,
    Act,
    Navigate,
    Record,
    Close,
}

impl Class {
    /// Every class, in the order a capability's display lists them.
    pub const ALL: [Class; 5] = [
        Class::Read,
        Class::Act,
        Class::Navigate,
        Class::Record,
        Class::Close,
    ];

    /// Its name: `read`, `act`, `navigate`, `record`, `close`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Read => "read",
            Class::Act => "act",
            Class::Navigate => "navigate",
            Class::Record => "record",
            Class::Close => "close",
        }
    }

    /// The class named `s`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// Who a holder caveat names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// A run, by its ID.
    Run(String),
    /// A grant of the grant flow, by its ID.
    Grant(String),
}

impl Holder {
    fn render(&self) -> String {
        match self {
            Holder::Run(id) => format!("run:{id}"),
            Holder::Grant(id) => format!("grant:{id}"),
        }
    }

    fn parse(s: &str) -> Option<Holder> {
        let (kind, id) = s.split_once(':')?;
        if id.is_empty() || id.chars().any(char::is_whitespace) {
            return None;
        }
        match kind {
            "run" => Some(Holder::Run(id.to_string())),
            "grant" => Some(Holder::Grant(id.to_string())),
            _ => None,
        }
    }
}

/// One first-party caveat (§18.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caveat {
    /// The session actions a call may be of.
    Actions(BTreeSet<Class>),
    /// The Unix time, in milliseconds, from which every call is refused.
    Expires(i64),
    /// Who may make a call.
    Holder(Holder),
    /// The origins a `navigate` call may reach, each `scheme://host[:port]`.
    Origin(BTreeSet<String>),
}

/// `s` as an origin when it is one, in its own serialization, or `None`.
#[must_use]
pub fn origin_of(s: &str) -> Option<String> {
    let u = url::Url::parse(s).ok()?;
    let o = u.origin();
    o.is_tuple().then(|| o.ascii_serialization())
}

impl Caveat {
    /// Its text, `name=value`.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Caveat::Actions(ops) => format!(
                "actions={}",
                ops.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(" ")
            ),
            Caveat::Expires(ms) => format!("expires={ms}"),
            Caveat::Holder(h) => format!("holder={}", h.render()),
            Caveat::Origin(o) => {
                format!("origin={}", o.iter().cloned().collect::<Vec<_>>().join(" "))
            }
        }
    }

    /// The caveat `s` is, or `None` for one this host does not understand:
    /// another name, or a value that does not parse.
    #[must_use]
    pub fn parse(s: &str) -> Option<Caveat> {
        let (name, value) = s.split_once('=')?;
        let words = || value.split(' ').filter(|w| !w.is_empty());
        match name {
            "actions" => words()
                .map(Class::parse)
                .collect::<Option<BTreeSet<_>>>()
                .map(Caveat::Actions),
            "expires" => value
                .parse::<i64>()
                .ok()
                .filter(|_| value.bytes().all(|b| b.is_ascii_digit()))
                .map(Caveat::Expires),
            "holder" => Holder::parse(value).map(Caveat::Holder),
            "origin" => {
                let set = words()
                    .map(|w| origin_of(w).filter(|o| o == w))
                    .collect::<Option<BTreeSet<_>>>()?;
                (!set.is_empty()).then_some(Caveat::Origin(set))
            }
            _ => None,
        }
    }
}

/// Where a `navigate` call goes, for the `origin` caveat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Not a `navigate` call.
    None,
    /// To this absolute URL.
    Url(String),
    /// Somewhere not known before the call (`goBack`, `goForward`).
    Unknown,
}

/// One call, as a capability's caveats are held against it.
#[derive(Clone, Debug)]
pub struct Use<'a> {
    pub holder: &'a Holder,
    /// Its class; `None` for `share` and `revoke`, which need none.
    pub class: Option<Class>,
    pub target: Target,
    /// The registry's clock, in Unix milliseconds.
    pub now_ms: i64,
}

/// Why a capability refused a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It has a caveat this host does not understand.
    NotUnderstood,
    /// Its holder caveats do not name the caller.
    Holder,
    /// The call's class is not among its `ops`; the classes it allows.
    Class(Class, Vec<Class>),
    /// It expired at this time (Unix milliseconds).
    Expired(i64),
    /// A `navigate` call's target is outside its `origin`s.
    Origin(Option<String>, Vec<String>),
    /// An ID of its chain was revoked.
    Revoked,
}

impl Refusal {
    /// What the script is told: never the capability itself.
    #[must_use]
    pub fn message(&self) -> String {
        let names = |cs: &[Class]| {
            if cs.is_empty() {
                "nothing".to_string()
            } else {
                cs.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
            }
        };
        match self {
            Refusal::NotUnderstood => {
                "this browser page's capability has a condition this host does not understand, so it allows nothing".into()
            }
            Refusal::Holder => "this browser page's capability is not held by this run".into(),
            Refusal::Class(c, allowed) => format!(
                "this browser page's capability does not allow {} calls (it allows {}); use a page that does",
                c.as_str(),
                names(allowed)
            ),
            Refusal::Expired(ms) => format!(
                "this browser page's capability expired at {}",
                shown_time(*ms)
            ),
            Refusal::Origin(target, allowed) => match target {
                Some(o) => format!(
                    "this browser page's capability may navigate only to {}, not {o}",
                    allowed.join(", ")
                ),
                None => format!(
                    "this browser page's capability may navigate only to {}: goBack and goForward are not allowed",
                    allowed.join(", ")
                ),
            },
            Refusal::Revoked => "this browser page's capability was revoked".into(),
        }
    }
}

/// `ms` as an RFC 3339 time, to the second, in UTC.
fn shown_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map_or_else(
        || ms.to_string(),
        |t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// A capability that verified (see the module docs).
#[derive(Clone, Debug)]
pub struct Cap {
    mac: Macaroon,
    session: String,
    run: String,
    chain: Vec<String>,
}

impl Cap {
    /// Serialized, with [`PREFIX`].
    #[must_use]
    pub fn encode(&self) -> String {
        self.mac.encode(PREFIX)
    }

    /// The ID of the session it is a capability of.
    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }

    /// The run that opened its session.
    #[must_use]
    pub fn run(&self) -> &str {
        &self.run
    }

    /// The capability IDs from its session's root to it.
    #[must_use]
    pub fn chain(&self) -> &[String] {
        &self.chain
    }

    /// Its own ID: the last of its chain.
    #[must_use]
    pub fn id(&self) -> &str {
        self.chain.last().map_or("", String::as_str)
    }

    /// Whether it is its session's root, the one `browser.open` answered.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.chain.len() == 1
    }

    /// Its caveats.
    ///
    /// # Errors
    /// [`Refusal::NotUnderstood`] when one does not parse.
    pub fn caveats(&self) -> Result<Vec<Caveat>, Refusal> {
        self.mac
            .caveats()
            .iter()
            .map(|c| Caveat::parse(c).ok_or(Refusal::NotUnderstood))
            .collect()
    }

    /// Whether its holder caveats admit `holder` (and every caveat parses).
    #[must_use]
    pub fn admits(&self, holder: &Holder) -> bool {
        self.caveats().is_ok_and(|cs| {
            cs.iter()
                .all(|c| !matches!(c, Caveat::Holder(h) if h != holder))
        })
    }

    /// Whether `revoked` holds an ID of its chain.
    #[must_use]
    pub fn revoked_in(&self, revoked: &BTreeSet<String>) -> bool {
        self.chain.iter().any(|id| revoked.contains(id))
    }

    /// Holds every caveat against `u` (see the module docs).
    ///
    /// # Errors
    /// The first caveat that refuses it, the holder's first.
    pub fn check(&self, u: &Use<'_>) -> Result<(), Refusal> {
        let cs = self.caveats()?;
        if !self.admits(u.holder) {
            return Err(Refusal::Holder);
        }
        for c in &cs {
            match c {
                Caveat::Holder(_) => {}
                Caveat::Expires(at) if u.now_ms >= *at => return Err(Refusal::Expired(*at)),
                Caveat::Expires(_) => {}
                Caveat::Actions(ops) => {
                    if let Some(class) = u.class
                        && !ops.contains(&class)
                    {
                        return Err(Refusal::Class(class, self.classes(&cs)));
                    }
                }
                Caveat::Origin(origins) => {
                    let allowed = || origins.iter().cloned().collect::<Vec<_>>();
                    match &u.target {
                        Target::None => {}
                        Target::Unknown => return Err(Refusal::Origin(None, allowed())),
                        Target::Url(url) => {
                            let o = origin_of(url);
                            if !o.as_ref().is_some_and(|o| origins.contains(o)) {
                                return Err(Refusal::Origin(
                                    Some(o.unwrap_or_else(|| "that URL".into())),
                                    allowed(),
                                ));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The classes every `ops` caveat of `cs` allows.
    fn classes(&self, cs: &[Caveat]) -> Vec<Class> {
        Class::ALL
            .into_iter()
            .filter(|c| {
                cs.iter()
                    .all(|cv| !matches!(cv, Caveat::Actions(ops) if !ops.contains(c)))
            })
            .collect()
    }

    /// It without its signature, as JSON `{"i": identifier, "c":
    /// [caveat, …]}`: what an export's grant row keeps (spec §18.5), from
    /// which [`Keys::signed`] makes it again with the instance key, so the
    /// row alone allows no call.
    #[must_use]
    pub fn unsigned(&self) -> String {
        serde_json::json!({"i": self.mac.identifier(), "c": self.mac.caveats()}).to_string()
    }

    /// Its earliest expiry, if it has one.
    #[must_use]
    pub fn expires(&self) -> Option<i64> {
        self.caveats().ok().and_then(|cs| {
            cs.iter()
                .filter_map(|c| match c {
                    Caveat::Expires(ms) => Some(*ms),
                    _ => None,
                })
                .min()
        })
    }

    /// What a script and people are shown of it (§18.5): `"<browser
    /// page>"` for one that narrows nothing but its holder, else its
    /// classes and expiry, `"<browser page: read; expires
    /// 2026-11-19T10:42:00Z>"`.
    #[must_use]
    pub fn shown(&self) -> String {
        let Ok(cs) = self.caveats() else {
            return "<browser page: nothing>".into();
        };
        let mut parts = Vec::new();
        if cs.iter().any(|c| matches!(c, Caveat::Actions(_))) {
            let classes = self.classes(&cs);
            parts.push(if classes.is_empty() {
                "nothing".to_string()
            } else {
                classes
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            });
        }
        if let Some(ms) = self.expires() {
            parts.push(format!("expires {}", shown_time(ms)));
        }
        if parts.is_empty() {
            "<browser page>".into()
        } else {
            format!("<browser page: {}>", parts.join("; "))
        }
    }
}

/// The instance key capabilities are made and checked with (§18.2). Its
/// `Debug` shows only its ID.
#[derive(Clone)]
pub struct Keys {
    instance: [u8; 32],
    id: String,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// A new capability ID: 16 random bytes, base64url.
fn new_id() -> String {
    base64url(&random_bytes::<16>())
}

impl Keys {
    /// Keys over the instance key `instance`.
    #[must_use]
    pub fn new(instance: [u8; 32]) -> Keys {
        let id = clax_core::secret::sha256_hex(&base64url(&instance))[..8].to_string();
        Keys { instance, id }
    }

    /// Keys over a random instance key, for a host without a KEK: what it
    /// makes verifies in this process alone.
    #[must_use]
    pub fn random() -> Keys {
        Keys::new(random_bytes::<32>())
    }

    /// The instance key's ID, the identifier's second part.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    fn session_key(&self, session: &str) -> [u8; 32] {
        derive_key(&self.instance, &format!("session/{session}"))
    }

    fn mint(&self, session: &str, run: &str, chain: Vec<String>, caveats: &[String]) -> Cap {
        let identifier = format!("{VERSION}/{}/{session}/{}/{run}", self.id, chain.join("."));
        let mut mac = Macaroon::mint(&self.session_key(session), &identifier);
        for c in caveats {
            mac = mac.attenuate(c);
        }
        Cap {
            mac,
            session: session.to_string(),
            run: run.to_string(),
            chain,
        }
    }

    /// The root capability of session `session`, opened by run `run`: held
    /// by that run alone.
    #[must_use]
    pub fn root(&self, session: &str, run: &str) -> Cap {
        let holder = Caveat::Holder(Holder::Run(run.to_string())).render();
        self.mint(session, run, vec![new_id()], &[holder])
    }

    /// A capability of `parent`'s session under a new ID of its chain,
    /// with `parent`'s caveats and `more` (§18.4): it allows no call
    /// `parent` does not.
    #[must_use]
    pub fn child(&self, parent: &Cap, more: &[Caveat]) -> Cap {
        let mut chain = parent.chain.clone();
        chain.push(new_id());
        let mut caveats = parent.mac.caveats().to_vec();
        caveats.extend(more.iter().map(Caveat::render));
        self.mint(&parent.session, &parent.run, chain, &caveats)
    }

    /// A capability of `parent`'s session under a new ID of its chain for
    /// another holder (§18.5): `parent`'s caveats but its holder ones,
    /// `holder`'s, and `more`. Only the host mints one, with the session's
    /// key: no holder can take a caveat off a macaroon.
    #[must_use]
    pub fn child_for(&self, parent: &Cap, holder: &Holder, more: &[Caveat]) -> Cap {
        let mut chain = parent.chain.clone();
        chain.push(new_id());
        let mut caveats: Vec<String> = parent
            .mac
            .caveats()
            .iter()
            .filter(|c| !matches!(Caveat::parse(c), Some(Caveat::Holder(_))))
            .cloned()
            .collect();
        caveats.push(Caveat::Holder(holder.clone()).render());
        caveats.extend(more.iter().map(Caveat::render));
        self.mint(&parent.session, &parent.run, chain, &caveats)
    }

    /// The capability `s` is, when it decodes, names this instance key and
    /// verifies with its session's key; else `None`, with nothing said of
    /// why (a forged one and one of another instance alike).
    #[must_use]
    pub fn verify(&self, s: &str) -> Option<Cap> {
        let mac = Macaroon::decode(PREFIX, s)?;
        let (session, run, chain) = self.parts(mac.identifier())?;
        mac.verify(&self.session_key(&session)).ok()?;
        Some(Cap {
            session,
            run,
            chain,
            mac,
        })
    }

    /// The capability [`Cap::unsigned`] answered, signed again with its
    /// session's key; `None` for JSON that is not one, or an identifier
    /// that names another instance key.
    #[must_use]
    pub fn signed(&self, unsigned: &str) -> Option<Cap> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Unsigned {
            i: String,
            c: Vec<String>,
        }
        let u: Unsigned = serde_json::from_str(unsigned).ok()?;
        let (session, run, chain) = self.parts(&u.i)?;
        Some(self.mint(&session, &run, chain, &u.c))
    }

    /// An identifier's session, run and chain, when it is of this
    /// instance key's version and ID.
    fn parts(&self, identifier: &str) -> Option<(String, String, Vec<String>)> {
        let mut parts = identifier.splitn(5, '/');
        let (v, key, session, chain, run) = (
            parts.next()?,
            parts.next()?,
            parts.next()?,
            parts.next()?,
            parts.next()?,
        );
        if v != VERSION || key != self.id || session.is_empty() || run.is_empty() {
            return None;
        }
        let chain: Vec<String> = chain.split('.').map(str::to_string).collect();
        if chain.iter().any(|id| id.len() != 22) {
            return None;
        }
        Some((session.to_string(), run.to_string(), chain))
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn keys() -> Keys {
        Keys::new([7; 32])
    }

    /// A leaked string's prefix says what it is: a browser capability is
    /// never read as the owner's browser-session cookie.
    #[test]
    fn an_unsigned_capability_is_signed_again_by_its_key_alone() {
        let root = keys().root("sess1", "run-1");
        let child = keys().child_for(&root, &Holder::Grant("g1".into()), &[ops(&[Class::Read])]);
        let again = keys().signed(&child.unsigned()).unwrap();
        assert_eq!(again.encode(), child.encode());
        assert!(!child.unsigned().contains(&child.encode()[PREFIX.len()..]));
        assert!(Keys::new([8; 32]).signed(&child.unsigned()).is_none());
        assert!(keys().signed("{}").is_none());
        assert!(keys().signed(&child.encode()).is_none());
    }

    #[test]
    fn the_prefix_is_not_the_owner_cookie_s() {
        assert_ne!(PREFIX, clax_store::browser_login::OWNER_SESSION_PREFIX);
        assert!(keys().root("sess1", "run-1").encode().starts_with(PREFIX));
    }

    fn run(id: &str) -> Holder {
        Holder::Run(id.to_string())
    }

    fn at<'a>(holder: &'a Holder, class: Option<Class>, target: Target, now_ms: i64) -> Use<'a> {
        Use {
            holder,
            class,
            target,
            now_ms,
        }
    }

    fn ops(cs: &[Class]) -> Caveat {
        Caveat::Actions(cs.iter().copied().collect())
    }

    fn origins(os: &[&str]) -> Caveat {
        Caveat::Origin(os.iter().map(|o| (*o).to_string()).collect())
    }

    /// Every use a test tries: each class and none, each holder, each kind
    /// of target, before and after a time.
    fn uses(holders: &[Holder]) -> Vec<Use<'_>> {
        let mut out = Vec::new();
        for h in holders {
            for class in Class::ALL.into_iter().map(Some).chain([None]) {
                for target in [
                    Target::None,
                    Target::Unknown,
                    Target::Url("https://a.example/x".into()),
                    Target::Url("https://b.example/".into()),
                ] {
                    for now in [0, 5_000, 10_000] {
                        out.push(at(h, class, target.clone(), now));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn a_root_verifies_and_is_its_runs_alone() {
        let k = keys();
        let root = k.root("sess1", "run-1");
        let back = k.verify(&root.encode()).unwrap();
        assert_eq!(
            (back.session(), back.run(), back.is_root()),
            ("sess1", "run-1", true)
        );
        assert_eq!(back.id().len(), 22);
        assert!(back.encode().starts_with(PREFIX));
        let r1 = run("run-1");
        assert_eq!(
            back.check(&at(&r1, Some(Class::Act), Target::None, 0)),
            Ok(())
        );
        let r2 = run("run-2");
        assert_eq!(
            back.check(&at(&r2, Some(Class::Read), Target::None, 0)),
            Err(Refusal::Holder)
        );
        assert!(!back.admits(&Holder::Grant("run-1".into())));
        assert_eq!(back.shown(), "<browser page>");
        assert_ne!(k.root("sess1", "run-1").encode(), root.encode());
    }

    /// A forged, tampered or foreign capability does not verify: another
    /// instance key, another session's key, a changed identifier or
    /// caveat, a caveat dropped.
    #[test]
    fn a_forged_or_tampered_capability_does_not_verify() {
        let k = keys();
        let root = k.root("sess1", "run-1");
        let child = k.child(&root, &[ops(&[Class::Read])]);
        assert!(Keys::new([8; 32]).verify(&child.encode()).is_none());
        let m = Macaroon::decode(PREFIX, &child.encode()).unwrap();
        // Re-signed with a key that is not the session's.
        let forged = Macaroon::mint(&[0; 32], m.identifier()).attenuate("holder=run:run-1");
        assert!(k.verify(&forged.encode(PREFIX)).is_none());
        // The same chain moved to another session.
        let moved = m.identifier().replace("/sess1/", "/sess2/");
        let mut re = Macaroon::mint(&[0; 32], &moved);
        for c in m.caveats() {
            re = re.attenuate(c);
        }
        assert!(k.verify(&re.encode(PREFIX)).is_none());
        // Its JSON edited: a caveat dropped, the run changed.
        let json = |f: &dyn Fn(&mut serde_json::Value)| {
            let body = &child.encode()[PREFIX.len()..];
            let raw =
                base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, body)
                    .unwrap();
            let mut v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            f(&mut v);
            format!("{PREFIX}{}", base64url(v.to_string().as_bytes()))
        };
        for bad in [
            json(&|v| {
                v["c"].as_array_mut().unwrap().pop();
            }),
            json(&|v| v["c"][1] = "actions=read act".into()),
            json(&|v| v["i"] = v["i"].as_str().unwrap().replace("run-1", "run-2").into()),
            "cxk_".into(),
            root.encode()[..root.encode().len() - 2].to_string(),
        ] {
            assert!(k.verify(&bad).is_none(), "{bad}");
        }
        assert!(k.verify(&child.encode()).is_some());
    }

    /// A child allows no use its parent refuses, whatever it adds.
    #[test]
    fn caveats_only_narrow() {
        let k = keys();
        let root = k.root("sess1", "run-1");
        let adds: Vec<Vec<Caveat>> = vec![
            vec![ops(&[Class::Read])],
            vec![
                ops(&[Class::Read, Class::Act]),
                ops(&[Class::Act, Class::Close]),
            ],
            vec![ops(&[])],
            vec![Caveat::Expires(5_000)],
            vec![origins(&["https://a.example"])],
            vec![Caveat::Holder(run("run-2"))],
            vec![Caveat::Holder(Holder::Grant("g_1".into()))],
            vec![ops(&Class::ALL), Caveat::Expires(i64::MAX)],
        ];
        let holders = [run("run-1"), run("run-2"), Holder::Grant("g_1".into())];
        let mut parents = vec![root.clone()];
        for add in &adds {
            for parent in parents.clone() {
                let child = k.child(&parent, add);
                assert!(k.verify(&child.encode()).is_some());
                for u in uses(&holders) {
                    if child.check(&u).is_ok() {
                        assert_eq!(parent.check(&u), Ok(()), "{add:?} widened {u:?}");
                    }
                }
                parents.push(child);
            }
        }
    }

    #[test]
    fn an_unknown_caveat_fails_closed() {
        let k = keys();
        let root = k.root("sess1", "run-1");
        let r1 = run("run-1");
        for c in [
            "budget=10",
            "actions=read write",
            "actions",
            "ops=read",
            "expires=soon",
            "expires=-1",
            "expires=+5",
            "holder=agent:x",
            "holder=run:",
            "origin=https://a.example/path",
            "origin=",
            "origin=example.com",
            "",
        ] {
            let mut m = Macaroon::decode(PREFIX, &root.encode()).unwrap();
            m = m.attenuate(c);
            let cap = k
                .verify(&m.encode(PREFIX))
                .expect("an appended caveat verifies");
            for u in uses(std::slice::from_ref(&r1)) {
                assert_eq!(cap.check(&u), Err(Refusal::NotUnderstood), "{c}");
            }
            assert!(!cap.admits(&r1), "{c}");
        }
    }

    #[test]
    fn expiry_is_on_the_clock_it_is_given() {
        let k = keys();
        let cap = k.child(&k.root("sess1", "run-1"), &[Caveat::Expires(10_000)]);
        let r1 = run("run-1");
        assert_eq!(
            cap.check(&at(&r1, Some(Class::Read), Target::None, 9_999)),
            Ok(())
        );
        assert_eq!(
            cap.check(&at(&r1, Some(Class::Read), Target::None, 10_000)),
            Err(Refusal::Expired(10_000))
        );
        assert_eq!(
            cap.check(&at(&r1, None, Target::None, 10_001)),
            Err(Refusal::Expired(10_000))
        );
        assert_eq!(cap.shown(), "<browser page: expires 1970-01-01T00:00:10Z>");
    }

    #[test]
    fn a_read_only_capability_cannot_act() {
        let k = keys();
        let cap = k.child(&k.root("sess1", "run-1"), &[ops(&[Class::Read])]);
        let r1 = run("run-1");
        assert_eq!(
            cap.check(&at(&r1, Some(Class::Read), Target::None, 0)),
            Ok(())
        );
        assert_eq!(cap.check(&at(&r1, None, Target::None, 0)), Ok(()));
        for c in [Class::Act, Class::Navigate, Class::Record, Class::Close] {
            let e = cap.check(&at(&r1, Some(c), Target::None, 0)).unwrap_err();
            assert_eq!(e, Refusal::Class(c, vec![Class::Read]));
            assert!(e.message().contains("it allows read"), "{}", e.message());
        }
        assert_eq!(cap.shown(), "<browser page: read>");
    }

    #[test]
    fn an_origin_caveat_holds_navigation_to_its_origins() {
        let k = keys();
        let cap = k.child(
            &k.root("sess1", "run-1"),
            &[origins(&["https://a.example"])],
        );
        let r1 = run("run-1");
        let nav = |t| cap.check(&at(&r1, Some(Class::Navigate), t, 0));
        assert_eq!(
            nav(Target::Url("https://a.example/deep?q=1".into())),
            Ok(())
        );
        assert!(matches!(
            nav(Target::Url("https://b.example/".into())),
            Err(Refusal::Origin(Some(_), _))
        ));
        assert!(matches!(
            nav(Target::Url("http://a.example/".into())),
            Err(Refusal::Origin(Some(_), _))
        ));
        assert!(matches!(
            nav(Target::Unknown),
            Err(Refusal::Origin(None, _))
        ));
        assert_eq!(
            cap.check(&at(&r1, Some(Class::Act), Target::None, 0)),
            Ok(())
        );
    }

    #[test]
    fn a_child_extends_its_parents_chain() {
        let k = keys();
        let root = k.root("sess1", "run-1");
        let a = k.child(&root, &[]);
        let b = k.child(&a, &[]);
        let c = k.child(&root, &[]);
        assert_eq!(b.chain()[..2], [root.id().to_string(), a.id().to_string()]);
        assert!(!a.is_root());
        let revoked: BTreeSet<String> = [a.id().to_string()].into();
        assert!(a.revoked_in(&revoked) && b.revoked_in(&revoked));
        assert!(!root.revoked_in(&revoked) && !c.revoked_in(&revoked));
    }

    /// A mint for another holder keeps every caveat but the holder, which
    /// it replaces: the run no longer holds it, the grant does, and it is
    /// no wider than its parent for anyone.
    #[test]
    fn a_mint_for_a_new_holder_keeps_every_caveat_but_the_holder() {
        let k = keys();
        let parent = k.child(
            &k.root("sess1", "run-1"),
            &[ops(&[Class::Read, Class::Act])],
        );
        let g = Holder::Grant("g_1".into());
        let out = k.child_for(&parent, &g, &[Caveat::Expires(10_000)]);
        assert!(k.verify(&out.encode()).is_some());
        assert_eq!(out.chain()[..2], parent.chain()[..]);
        assert!(out.admits(&g) && !out.admits(&run("run-1")));
        assert_eq!(
            out.check(&at(&g, Some(Class::Read), Target::None, 0)),
            Ok(())
        );
        assert!(
            out.check(&at(&g, Some(Class::Navigate), Target::None, 0))
                .is_err()
        );
        assert!(
            out.check(&at(&g, Some(Class::Read), Target::None, 10_000))
                .is_err()
        );
    }

    #[test]
    fn caveats_render_as_they_parse() {
        for c in [
            ops(&[Class::Read, Class::Close]),
            ops(&[]),
            Caveat::Expires(12),
            Caveat::Holder(run("r_1")),
            Caveat::Holder(Holder::Grant("g_1".into())),
            origins(&["https://a.example", "http://b.example:8080"]),
        ] {
            assert_eq!(
                Caveat::parse(&c.render()),
                Some(c.clone()),
                "{}",
                c.render()
            );
        }
    }
}

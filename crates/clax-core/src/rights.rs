//! Access rights (spec §7): GNAP (RFC 9635) access objects with Clax types,
//! actions and locations, the reference presets, and the effective rights of
//! an agent. Evaluation is pure; `own` (artifacts the agent's person created)
//! is resolved by the caller at check time through a predicate.
//!
//! How routes check (spec §7.5):
//! - A route whose path names an artifact asks [`Rights::allows`] with it.
//! - A route that names none (`create`, a session's feedback, working or
//!   watches, `ask`) asks [`Rights::allows_somewhere`]: whether the action is
//!   granted at any location. `allows(.., None, ..)` holds only at `*`.
//! - A listing asks [`Rights::allows`] per item and leaves out what fails.
//! - The db level for an artifact is per action: [`Rights::db_level`] with
//!   `"write"` for a write, `"read"` for a read.
//!
//! Limits: a client's array holds at most [`MAX_ACCESS`] access objects,
//! counted before merging, with a preset name counting as all of its
//! objects (`clax:collaborator` alone is nine); each object holds at most
//! [`MAX_LOCATIONS`] locations. Merged rights hold at most one object per
//! type, `clax-db` level and action set: 86 in all (15 artifact action sets,
//! 63 comment, 7 db, 1 ask).

use crate::db::Level;
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};

/// The reader preset: read everything, reply and watch, ask.
pub const PRESET_READER: &str = "clax:reader";
/// The collaborator preset, a person's default.
pub const PRESET_COLLABORATOR: &str = "clax:collaborator";
/// Every action everywhere, db at `admin`.
pub const PRESET_FULL: &str = "clax:full";

/// Actions of `clax-artifacts`.
pub const ARTIFACT_ACTIONS: &[&str] = &["read", "create", "update", "delete"];
/// Actions of `clax-comments`.
pub const COMMENT_ACTIONS: &[&str] = &["read", "write", "reply", "resolve", "send", "watch"];
/// Actions of `clax-db`.
pub const DB_ACTIONS: &[&str] = &["read", "write"];
/// Actions of `clax-ask`.
pub const ASK_ACTIONS: &[&str] = &["ask"];

/// Most access objects in one rights array a client sends. Effective rights
/// are a product of up to three such arrays, so this bounds their rebuild.
pub const MAX_ACCESS: usize = 16;
/// Most locations in one access object a client sends.
pub const MAX_LOCATIONS: usize = 16;
/// Longest artifact ID accepted as a location, in bytes.
pub const MAX_LOCATION_BYTES: usize = 64;

const INVALID: &str = "invalid_rights";
/// The form [`Location::OwnArtifact`] takes in [`Rights::to_json`]. Only
/// `Rights::parse_internal` (the store's reader) reads it back; clients cannot send it.
const OWN_PREFIX: &str = "own:";

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::invalid(INVALID, message)
}

/// The type of an access object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RightType {
    #[serde(rename = "clax-artifacts")]
    Artifacts,
    #[serde(rename = "clax-comments")]
    Comments,
    #[serde(rename = "clax-db")]
    Db,
    #[serde(rename = "clax-ask")]
    Ask,
}

impl RightType {
    pub fn parse(s: &str) -> Option<RightType> {
        match s {
            "clax-artifacts" => Some(RightType::Artifacts),
            "clax-comments" => Some(RightType::Comments),
            "clax-db" => Some(RightType::Db),
            "clax-ask" => Some(RightType::Ask),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RightType::Artifacts => "clax-artifacts",
            RightType::Comments => "clax-comments",
            RightType::Db => "clax-db",
            RightType::Ask => "clax-ask",
        }
    }

    /// Every action this type grants.
    pub fn actions(self) -> &'static [&'static str] {
        match self {
            RightType::Artifacts => ARTIFACT_ACTIONS,
            RightType::Comments => COMMENT_ACTIONS,
            RightType::Db => DB_ACTIONS,
            RightType::Ask => ASK_ACTIONS,
        }
    }

    /// Whether objects of this type carry locations. `clax-ask` does not: a
    /// question is not asked at an artifact.
    fn located(self) -> bool {
        self != RightType::Ask
    }
}

/// Where an access object applies. Clients send `"*"`, `"own"` or an
/// artifact ID (spec §7.1); [`Location::OwnArtifact`] only arises from
/// intersecting rights. Its serde form is the string; deserializing reads a
/// client's location.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Location {
    /// `"*"`: every artifact.
    Any,
    /// `"own"`: artifacts the agent's person created, resolved at check time.
    Own,
    /// An artifact ID.
    Artifact(String),
    /// The artifact, only while it is the person's own: what `own ∩ <id>`
    /// leaves. Ownership is still resolved at check time. Written
    /// `"own:<id>"` by [`Rights::to_json`].
    OwnArtifact(String),
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_LOCATION_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Location {
    /// Parses a client's location: `"*"`, `"own"` or an artifact ID.
    pub fn parse(s: &str) -> Result<Location> {
        Location::parse_with(s, false)
    }

    /// `internal` also reads `"own:<id>"`, as [`Rights::to_json`] writes it.
    fn parse_with(s: &str, internal: bool) -> Result<Location> {
        match s {
            "*" => Ok(Location::Any),
            "own" => Ok(Location::Own),
            _ if valid_id(s) => Ok(Location::Artifact(s.to_string())),
            _ => match s.strip_prefix(OWN_PREFIX) {
                Some(id) if internal && valid_id(id) => Ok(Location::OwnArtifact(id.to_string())),
                _ => Err(invalid(format!(
                    "location '{s}' is not \"*\", \"own\" or an artifact ID"
                ))),
            },
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Location::Any => "*".to_string(),
            Location::Own => "own".to_string(),
            Location::Artifact(id) => id.clone(),
            Location::OwnArtifact(id) => format!("{OWN_PREFIX}{id}"),
        }
    }

    /// Whether this location covers `artifact` (`None`: no artifact, which
    /// only `*` covers).
    pub fn matches(&self, artifact: Option<&str>, own: &dyn Fn(&str) -> bool) -> bool {
        match (self, artifact) {
            (Location::Any, _) => true,
            (_, None) => false,
            (Location::Own, Some(a)) => own(a),
            (Location::Artifact(id), Some(a)) => id == a,
            (Location::OwnArtifact(id), Some(a)) => id == a && own(a),
        }
    }

    /// Where both locations apply, or `None` where nowhere.
    fn and(&self, other: &Location) -> Option<Location> {
        use Location::*;
        match (self, other) {
            (Any, x) | (x, Any) => Some(x.clone()),
            (Own, Own) => Some(Own),
            (Own, Artifact(id) | OwnArtifact(id)) | (Artifact(id) | OwnArtifact(id), Own) => {
                Some(OwnArtifact(id.clone()))
            }
            (Artifact(a), Artifact(b)) if a == b => Some(Artifact(a.clone())),
            (Artifact(a) | OwnArtifact(a), Artifact(b) | OwnArtifact(b)) if a == b => {
                Some(OwnArtifact(a.clone()))
            }
            _ => None,
        }
    }

    /// Whether `other` applies everywhere this location does, whoever owns
    /// what.
    fn within(&self, other: &Location) -> bool {
        use Location::*;
        match (self, other) {
            (_, Any) => true,
            (Own, Own) => true,
            (OwnArtifact(_), Own) => true,
            (Artifact(a) | OwnArtifact(a), Artifact(b)) => a == b,
            (OwnArtifact(a), OwnArtifact(b)) => a == b,
            _ => false,
        }
    }
}

impl TryFrom<String> for Location {
    type Error = CoreError;
    fn try_from(s: String) -> Result<Self> {
        Location::parse(&s)
    }
}

impl From<Location> for String {
    fn from(l: Location) -> String {
        l.as_string()
    }
}

/// One access object. Its `actions` are never empty and are actions of
/// `kind`; `locations` are never empty, sorted and deduplicated (`[Any]`
/// for `clax-ask`); `level` is set exactly for `clax-db`, and is above
/// `view` when the object grants `write`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    kind: RightType,
    actions: BTreeSet<String>,
    locations: Vec<Location>,
    level: Option<Level>,
}

impl Access {
    fn new(
        kind: RightType,
        actions: &[&str],
        locations: Vec<Location>,
        level: Option<Level>,
    ) -> Access {
        Access {
            kind,
            actions: actions.iter().map(|a| a.to_string()).collect(),
            locations: canonical_locations(locations),
            level,
        }
    }

    pub fn kind(&self) -> RightType {
        self.kind
    }

    pub fn actions(&self) -> &BTreeSet<String> {
        &self.actions
    }

    pub fn locations(&self) -> &[Location] {
        &self.locations
    }

    pub fn level(&self) -> Option<Level> {
        self.level
    }

    fn parse(v: &Value, internal: bool) -> Result<Access> {
        let obj = v
            .as_object()
            .ok_or_else(|| invalid("an access object must be an object or a preset name"))?;
        if let Some(k) = obj
            .keys()
            .find(|k| !matches!(k.as_str(), "type" | "actions" | "locations" | "level"))
        {
            return Err(invalid(format!("unknown access field '{k}'")));
        }
        let kind = obj
            .get("type")
            .and_then(Value::as_str)
            .and_then(RightType::parse)
            .ok_or_else(|| {
                invalid("type must be clax-artifacts, clax-comments, clax-db or clax-ask")
            })?;
        let actions = obj
            .get("actions")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid(format!("{} needs a non-empty actions array", kind.as_str())))?
            .iter()
            .map(|a| match a.as_str() {
                Some(s) if kind.actions().contains(&s) => Ok(s.to_string()),
                _ => Err(invalid(format!(
                    "{a} is not an action of {} ({})",
                    kind.as_str(),
                    kind.actions().join(", ")
                ))),
            })
            .collect::<Result<BTreeSet<String>>>()?;
        let locations = match obj.get("locations") {
            None => vec![Location::Any],
            Some(Value::Array(ls)) if !ls.is_empty() && (internal || ls.len() <= MAX_LOCATIONS) => {
                ls.iter()
                    .map(|l| {
                        l.as_str()
                            .ok_or_else(|| invalid("a location must be a string"))
                            .and_then(|s| Location::parse_with(s, internal))
                    })
                    .collect::<Result<Vec<Location>>>()?
            }
            Some(_) => {
                return Err(invalid(format!(
                    "locations must be an array of 1 to {MAX_LOCATIONS} strings"
                )));
            }
        };
        let locations = canonical_locations(locations);
        if !kind.located() && locations != [Location::Any] {
            return Err(invalid("clax-ask takes no locations"));
        }
        if kind == RightType::Artifacts
            && actions.contains("create")
            && locations != [Location::Any]
        {
            return Err(invalid(
                "create is not at an artifact: grant it at \"*\" in an object of its own",
            ));
        }
        let level = match (kind, obj.get("level")) {
            (RightType::Db, None) => Some(if actions.contains("write") {
                Level::Interact
            } else {
                Level::View
            }),
            (RightType::Db, Some(l)) => match l.as_str().and_then(Level::parse) {
                Some(Level::View) if actions.contains("write") => {
                    return Err(invalid("write needs level interact or admin"));
                }
                Some(l @ (Level::View | Level::Interact | Level::Admin)) => Some(l),
                _ => {
                    return Err(invalid(format!("level {l} is not view, interact or admin")));
                }
            },
            (_, None) => None,
            (_, Some(_)) => return Err(invalid("only clax-db takes a level")),
        };
        Ok(Access {
            kind,
            actions,
            locations,
            level,
        })
    }

    fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("type".into(), json!(self.kind.as_str()));
        o.insert("actions".into(), json!(self.actions));
        if self.kind.located() {
            let ls: Vec<String> = self.locations.iter().map(Location::as_string).collect();
            o.insert("locations".into(), json!(ls));
        }
        if let Some(l) = self.level {
            o.insert("level".into(), json!(l.as_str()));
        }
        Value::Object(o)
    }

    fn covers(&self, artifact: Option<&str>, own: &dyn Fn(&str) -> bool) -> bool {
        self.locations.iter().any(|l| l.matches(artifact, own))
    }

    /// What both objects allow, or `None` where they share nothing.
    fn and(&self, other: &Access) -> Option<Access> {
        if self.kind != other.kind {
            return None;
        }
        let actions: BTreeSet<String> =
            self.actions.intersection(&other.actions).cloned().collect();
        if actions.is_empty() {
            return None;
        }
        let locations: Vec<Location> = self
            .locations
            .iter()
            .flat_map(|a| other.locations.iter().filter_map(move |b| a.and(b)))
            .collect();
        if locations.is_empty() {
            return None;
        }
        Some(Access {
            kind: self.kind,
            actions,
            locations: canonical_locations(locations),
            level: self.level.min(other.level),
        })
    }
}

/// Sorted, deduplicated, `[Any]` when any location is `*`, and without an
/// `own:<id>` that `own` or `<id>` already covers.
fn canonical_locations(mut ls: Vec<Location>) -> Vec<Location> {
    if ls.contains(&Location::Any) {
        return vec![Location::Any];
    }
    ls.sort();
    ls.dedup();
    let own = ls.contains(&Location::Own);
    let ids: std::collections::HashSet<String> = ls
        .iter()
        .filter_map(|l| match l {
            Location::Artifact(id) => Some(id.clone()),
            _ => None,
        })
        .collect();
    ls.retain(|l| match l {
        Location::OwnArtifact(id) => !own && !ids.contains(id),
        _ => true,
    });
    ls
}

/// A rights array: what is allowed is what any one access object allows.
/// Objects sharing type, level and locations, or type, level and actions,
/// are merged into one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rights(Vec<Access>);

impl Rights {
    /// Parses a client's GNAP access array, whose items are access objects
    /// or preset names, or a single preset name. Anything outside the spec's
    /// vocabulary is 400 `invalid_rights`, as are more than [`MAX_ACCESS`]
    /// objects or [`MAX_LOCATIONS`] locations in one object, `create` at a
    /// location other than `"*"`, and `write` at level `view`.
    ///
    /// A `clax-db` object without `level` gets `interact` when it grants
    /// `write`, else `view`; a narrowing that omits it may therefore lower
    /// an `admin` level to `interact`.
    pub fn parse(v: &Value) -> Result<Rights> {
        Rights::parse_with(v, false)
    }

    /// Reads back any [`Rights::to_json`], computed rights included (their
    /// `"own:<id>"` locations and unbounded sizes): rights the store wrote.
    pub(crate) fn parse_internal(v: &Value) -> Result<Rights> {
        Rights::parse_with(v, true)
    }

    fn parse_with(v: &Value, internal: bool) -> Result<Rights> {
        let items = match v {
            Value::String(name) => return preset_or_invalid(name),
            Value::Array(items) => items,
            _ => return Err(invalid("rights must be an array of access objects")),
        };
        let mut out = Vec::new();
        for item in items {
            match item {
                Value::String(name) => out.extend(preset_or_invalid(name)?.0),
                _ => out.push(Access::parse(item, internal)?),
            }
            if !internal && out.len() > MAX_ACCESS {
                return Err(invalid(format!("at most {MAX_ACCESS} access objects")));
            }
        }
        Ok(Rights(merge(out)))
    }

    /// A preset by name ([`PRESET_READER`], [`PRESET_COLLABORATOR`],
    /// [`PRESET_FULL`]).
    pub fn preset(name: &str) -> Option<Rights> {
        use Location::{Any, Own};
        use RightType::*;
        let reader = || {
            vec![
                Access::new(Artifacts, &["read"], vec![Any], None),
                Access::new(Comments, &["read", "reply", "watch"], vec![Any], None),
                Access::new(Db, &["read"], vec![Any], Some(Level::View)),
                Access::new(Ask, ASK_ACTIONS, vec![Any], None),
            ]
        };
        let rights = match name {
            PRESET_READER => reader(),
            PRESET_COLLABORATOR => {
                let mut r = reader();
                r.extend([
                    Access::new(Artifacts, &["create"], vec![Any], None),
                    Access::new(Artifacts, &["update", "delete"], vec![Own], None),
                    Access::new(Comments, &["write", "resolve", "send"], vec![Any], None),
                    Access::new(Db, DB_ACTIONS, vec![Any], Some(Level::Interact)),
                    Access::new(Db, DB_ACTIONS, vec![Own], Some(Level::Admin)),
                ]);
                r
            }
            PRESET_FULL => vec![
                Access::new(Artifacts, ARTIFACT_ACTIONS, vec![Any], None),
                Access::new(Comments, COMMENT_ACTIONS, vec![Any], None),
                Access::new(Db, DB_ACTIONS, vec![Any], Some(Level::Admin)),
                Access::new(Ask, ASK_ACTIONS, vec![Any], None),
            ],
            _ => return None,
        };
        Some(Rights(merge(rights)))
    }

    /// `clax:full`, the owner's rights.
    pub fn full() -> Rights {
        Rights::preset(PRESET_FULL).unwrap_or_default()
    }

    /// The access objects.
    pub fn iter(&self) -> impl Iterator<Item = &Access> {
        self.0.iter()
    }

    /// Whether these rights allow nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// What both allow: an action at a location only where both allow it,
    /// its db level the lower of the two.
    pub fn intersect(&self, other: &Rights) -> Rights {
        let out = self
            .0
            .iter()
            .flat_map(|a| other.0.iter().filter_map(move |b| a.and(b)))
            .collect();
        Rights(merge(out))
    }

    /// What either allows.
    pub fn union(&self, other: &Rights) -> Rights {
        Rights(merge(self.0.iter().chain(&other.0).cloned().collect()))
    }

    /// Whether `other` allows every action this does, wherever this does,
    /// at least at this db level for that action, whoever owns what.
    pub fn is_narrower_or_equal(&self, other: &Rights) -> bool {
        self.0.iter().all(|a| {
            a.locations.iter().all(|l| {
                a.actions.iter().all(|act| {
                    other.0.iter().any(|b| {
                        b.kind == a.kind
                            && b.actions.contains(act)
                            && b.level >= a.level
                            && b.locations.iter().any(|m| l.within(m))
                    })
                })
            })
        })
    }

    /// Whether these rights allow `action` of `kind` at `artifact` (`None`
    /// for an action at no artifact, such as `create`, which only `*`
    /// covers; `clax-ask` ignores the artifact). `own` says whether the
    /// agent's person created an artifact.
    pub fn allows(
        &self,
        kind: RightType,
        action: &str,
        artifact: Option<&str>,
        own: &dyn Fn(&str) -> bool,
    ) -> bool {
        self.0.iter().any(|a| {
            a.kind == kind
                && a.actions.contains(action)
                && (!kind.located() || a.covers(artifact, own))
        })
    }

    /// Whether these rights allow `action` of `kind` at any location: the
    /// check for routes that name no artifact (a session's feedback, working
    /// and watches). Their results are then filtered per artifact with
    /// [`Rights::allows`].
    pub fn allows_somewhere(&self, kind: RightType, action: &str) -> bool {
        self.0
            .iter()
            .any(|a| a.kind == kind && a.actions.contains(action))
    }

    /// The db level for `action` (`"read"` or `"write"`) at `artifact`: the
    /// highest of the `clax-db` objects that grant that action there, or
    /// `None` when none does.
    pub fn db_level(
        &self,
        artifact: &str,
        action: &str,
        own: &dyn Fn(&str) -> bool,
    ) -> Option<Level> {
        self.0
            .iter()
            .filter(|a| {
                a.kind == RightType::Db
                    && a.actions.contains(action)
                    && a.covers(Some(artifact), own)
            })
            .filter_map(|a| a.level)
            .max()
    }

    /// The GNAP access array. [`Rights::parse`] reads back rights a client
    /// could send; computed rights need `Rights::parse_internal` (the store's reader).
    pub fn to_json(&self) -> Value {
        Value::Array(self.0.iter().map(Access::to_json).collect())
    }
}

fn preset_or_invalid(name: &str) -> Result<Rights> {
    Rights::preset(name).ok_or_else(|| {
        invalid(format!(
            "unknown preset '{name}' ({PRESET_READER}, {PRESET_COLLABORATOR}, {PRESET_FULL})"
        ))
    })
}

/// Merges objects that share type, level and locations (uniting their
/// actions) and objects that share type, level and actions (uniting their
/// locations), until neither applies. Each object allows what it allowed
/// before, so the rights are unchanged; the result has at most one object
/// per type, level and action set, and is its own merge.
fn merge(mut items: Vec<Access>) -> Vec<Access> {
    loop {
        let before = items.len();
        items = merge_on(
            items,
            |a| (a.kind, a.level, a.locations.clone()),
            |into, a| into.actions.extend(a.actions),
        );
        items = merge_on(
            items,
            |a| (a.kind, a.level, a.actions.clone()),
            |into, a| {
                into.locations.extend(a.locations);
                into.locations = canonical_locations(std::mem::take(&mut into.locations));
            },
        );
        if items.len() == before {
            return items;
        }
    }
}

/// One merging pass, linear in the objects: each object joins the first
/// one with its key, which keeps its place.
fn merge_on<K: std::hash::Hash + Eq>(
    items: Vec<Access>,
    key: impl Fn(&Access) -> K,
    join: impl Fn(&mut Access, Access),
) -> Vec<Access> {
    let mut at: HashMap<K, usize> = HashMap::with_capacity(items.len());
    let mut out: Vec<Access> = Vec::with_capacity(items.len());
    for a in items {
        let k = key(&a);
        match at.get(&k) {
            Some(&i) => join(&mut out[i], a),
            None => {
                at.insert(k, out.len());
                out.push(a);
            }
        }
    }
    out
}

/// An agent's effective rights (spec §7.2): `(person ∩ grant narrowing ∩
/// session narrowing) ∪ owner elevation`. A missing narrowing is no
/// narrowing; the elevation may exceed `person`.
pub fn effective(
    person: &Rights,
    grant_narrow: Option<&Rights>,
    session_narrow: Option<&Rights>,
    elevate: Option<&Rights>,
) -> Rights {
    let mut r = person.clone();
    for n in [grant_narrow, session_narrow].into_iter().flatten() {
        r = r.intersect(n);
    }
    match elevate {
        Some(e) => r.union(e),
        None => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn own(a: &str) -> bool {
        a == "mine"
    }

    #[test]
    fn presets_parse_and_allow_what_the_spec_says() {
        let c = Rights::parse(&json!(["clax:collaborator"])).unwrap();
        assert!(c.allows(RightType::Artifacts, "read", Some("other"), &own));
        assert!(c.allows(RightType::Artifacts, "create", None, &own));
        assert!(c.allows(RightType::Artifacts, "update", Some("mine"), &own));
        assert!(!c.allows(RightType::Artifacts, "update", Some("other"), &own));
        assert!(!c.allows(RightType::Artifacts, "delete", Some("other"), &own));
        assert!(c.allows(RightType::Comments, "send", Some("other"), &own));
        assert_eq!(c.db_level("other", "write", &own), Some(Level::Interact));
        assert_eq!(c.db_level("mine", "write", &own), Some(Level::Admin));
        let r = Rights::preset(PRESET_READER).unwrap();
        assert!(!r.allows(RightType::Comments, "resolve", Some("x"), &own));
        assert!(!r.allows(RightType::Comments, "write", Some("x"), &own));
        assert_eq!(serde_json::to_value(Location::Own).unwrap(), json!("own"));
        assert_eq!(
            serde_json::from_value::<Location>(json!("*")).unwrap(),
            Location::Any
        );
        assert!(serde_json::from_value::<Location>(json!("own:abc")).is_err());
        assert!(Rights::full().allows(RightType::Artifacts, "delete", Some("x"), &own));
    }

    #[test]
    fn effective_is_person_narrowed_then_elevated() {
        let person = Rights::preset(PRESET_COLLABORATOR).unwrap();
        let no_delete = Rights::parse(&json!([
            {"type": "clax-artifacts", "actions": ["read", "create", "update"], "locations": ["*"]},
            {"type": "clax-comments", "actions": ["read", "write", "reply", "resolve", "send", "watch"], "locations": ["*"]},
            {"type": "clax-db", "actions": ["read", "write"], "level": "admin", "locations": ["*"]},
            {"type": "clax-ask", "actions": ["ask"]}]))
        .unwrap();
        let up = Rights::parse(
            &json!([{"type": "clax-artifacts", "actions": ["update"], "locations": ["qr"]}]),
        )
        .unwrap();
        let e = effective(&person, Some(&no_delete), None, Some(&up));
        assert!(
            !e.allows(RightType::Artifacts, "delete", Some("mine"), &own),
            "narrowed away"
        );
        assert!(
            e.allows(RightType::Artifacts, "update", Some("qr"), &own),
            "elevated past the person"
        );
        assert!(!e.allows(RightType::Artifacts, "update", Some("other"), &own));
        assert_eq!(
            e.db_level("other", "write", &own),
            Some(Level::Interact),
            "level takes the lower"
        );
    }

    #[test]
    fn narrowing_is_checked_and_junk_is_refused() {
        let c = Rights::preset(PRESET_COLLABORATOR).unwrap();
        assert!(
            Rights::preset(PRESET_READER)
                .unwrap()
                .is_narrower_or_equal(&c)
        );
        assert!(!Rights::full().is_narrower_or_equal(&c));
        for bad in [
            json!("clax:nope"),
            json!([{"type": "clax-live", "actions": ["read"]}]),
            json!([{"type": "clax-artifacts", "actions": ["fly"]}]),
            json!([{"type": "clax-db", "actions": ["write"], "level": "owner"}]),
        ] {
            assert!(
                matches!(Rights::parse(&bad), Err(crate::CoreError::Invalid { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn round_trips_as_gnap_access_objects() {
        let c = Rights::preset(PRESET_COLLABORATOR).unwrap();
        assert_eq!(Rights::parse(&c.to_json()).unwrap(), c);
    }

    #[test]
    fn own_and_an_id_stay_a_conjunction_resolved_at_check_time() {
        let person = Rights::preset(PRESET_COLLABORATOR).unwrap();
        let narrow = Rights::parse(&json!([
            {"type": "clax-artifacts", "actions": ["read", "update"], "locations": ["mine", "other"]}
        ]))
        .unwrap();
        let e = effective(&person, None, Some(&narrow), None);
        assert!(e.allows(RightType::Artifacts, "update", Some("mine"), &own));
        assert!(!e.allows(RightType::Artifacts, "update", Some("other"), &own));
        assert!(e.allows(RightType::Artifacts, "read", Some("other"), &own));
        assert!(!e.allows(RightType::Artifacts, "read", Some("third"), &own));
        let now_theirs = |_: &str| false;
        assert!(!e.allows(RightType::Artifacts, "update", Some("mine"), &now_theirs));
        assert!(e.is_narrower_or_equal(&person));
        assert!(!person.is_narrower_or_equal(&e));
        assert_eq!(Rights::parse_internal(&e.to_json()).unwrap(), e);
        assert!(
            Rights::parse(&e.to_json()).is_err(),
            "computed own:<id> is not client vocabulary"
        );
    }

    #[test]
    fn missing_narrowings_narrow_nothing_and_ask_needs_no_artifact() {
        let person = Rights::preset(PRESET_READER).unwrap();
        assert_eq!(effective(&person, None, None, None), person);
        assert!(person.allows(RightType::Ask, "ask", None, &own));
        assert_eq!(person.db_level("x", "read", &own), Some(Level::View));
        assert_eq!(person.db_level("x", "write", &own), None);
        assert_eq!(
            Rights::parse(&json!([]))
                .unwrap()
                .db_level("x", "read", &own),
            None
        );
        for bad in [
            json!([{"type": "clax-ask", "actions": ["ask"], "locations": ["x"]}]),
            json!([{"type": "clax-comments", "actions": ["read"], "level": "view"}]),
            json!([{"type": "clax-db", "actions": []}]),
            json!([{"type": "clax-db", "actions": ["read"], "locations": ["a b"]}]),
            json!([{"type": "clax-db", "actions": ["read"], "privileges": ["x"]}]),
            json!({"type": "clax-db", "actions": ["read"]}),
            json!([{"type": "clax-db", "actions": ["read"], "locations": ["own:abc"]}]),
            json!([{"type": "clax-artifacts", "actions": ["read", "create"], "locations": ["own"]}]),
            json!([{"type": "clax-db", "actions": ["write"], "level": "view"}]),
            json!(vec![
                json!({"type": "clax-ask", "actions": ["ask"]});
                MAX_ACCESS + 1
            ]),
            json!([{"type": "clax-db", "actions": ["read"],
                    "locations": (0..=MAX_LOCATIONS).map(|i| format!("a{i}")).collect::<Vec<_>>()}]),
        ] {
            assert!(
                matches!(
                    Rights::parse(&bad),
                    Err(crate::CoreError::Invalid {
                        code: "invalid_rights",
                        ..
                    })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn an_elevated_read_level_never_lends_itself_to_write() {
        let person = Rights::preset(PRESET_COLLABORATOR).unwrap();
        let up = Rights::parse(&json!([
            {"type": "clax-db", "actions": ["read"], "level": "admin", "locations": ["qr"]}
        ]))
        .unwrap();
        let e = effective(&person, None, None, Some(&up));
        assert!(e.allows(RightType::Db, "write", Some("qr"), &own));
        assert_eq!(e.db_level("qr", "read", &own), Some(Level::Admin));
        assert_eq!(e.db_level("qr", "write", &own), Some(Level::Interact));
        assert!(
            !up.is_narrower_or_equal(&person),
            "admin read exceeds the person"
        );
        let read_admin_write_interact = Rights::parse(&json!([
            {"type": "clax-db", "actions": ["read"], "level": "admin", "locations": ["*"]},
            {"type": "clax-db", "actions": ["write"], "level": "interact", "locations": ["*"]}
        ]))
        .unwrap();
        let write_admin = Rights::parse(&json!([
            {"type": "clax-db", "actions": ["write"], "level": "admin", "locations": ["*"]}
        ]))
        .unwrap();
        assert!(!write_admin.is_narrower_or_equal(&read_admin_write_interact));
    }

    #[test]
    fn routes_without_an_artifact_ask_whether_the_action_is_granted_anywhere() {
        let narrow = Rights::parse(&json!([
            {"type": "clax-comments", "actions": ["watch"], "locations": ["abc"]}
        ]))
        .unwrap();
        assert!(narrow.allows_somewhere(RightType::Comments, "watch"));
        assert!(!narrow.allows(RightType::Comments, "watch", None, &own));
        assert!(!narrow.allows_somewhere(RightType::Comments, "read"));
        assert!(!narrow.allows_somewhere(RightType::Db, "read"));
        assert!(!Rights::default().allows_somewhere(RightType::Ask, "ask"));
    }

    /// A client array at the limits that no merge shrinks: every object has
    /// its own action set (four or more of the six comment actions, so every
    /// pair shares some) and its own locations. `locations(i)` gives object
    /// `i`'s.
    fn unmergeable(locations: impl Fn(usize) -> Vec<String>) -> Rights {
        let sets: Vec<Vec<&str>> = (0u32..64)
            .filter(|m| m.count_ones() >= 4)
            .map(|m| {
                (0..6)
                    .filter(|i| m & (1 << i) != 0)
                    .map(|i| COMMENT_ACTIONS[i])
                    .collect()
            })
            .collect();
        let objects: Vec<Value> = sets
            .iter()
            .take(MAX_ACCESS)
            .enumerate()
            .map(|(i, actions)| {
                json!({"type": "clax-comments", "actions": actions, "locations": locations(i)})
            })
            .collect();
        let r = Rights::parse(&Value::Array(objects)).unwrap();
        assert_eq!(r.iter().count(), MAX_ACCESS, "nothing merged");
        r
    }

    fn ids(tag: &str, i: usize, n: usize) -> Vec<String> {
        (0..n).map(|j| format!("{tag}{i}x{j}")).collect()
    }

    fn with_own(mut ls: Vec<String>) -> Vec<String> {
        ls.insert(0, "own".into());
        ls
    }

    /// Each object's IDs are the shared pool of `MAX_LOCATIONS + 1` minus
    /// one, so every pair across arrays shares all but two.
    fn pool(i: usize) -> Vec<String> {
        (0..=MAX_LOCATIONS)
            .filter(|&j| j != i)
            .map(|j| format!("p{j}"))
            .collect()
    }

    #[test]
    fn the_worst_rebuilds_at_the_limits_are_bounded() {
        let n = MAX_LOCATIONS;
        let shapes: Vec<(&str, [Rights; 4])> = vec![
            (
                "own and distinct IDs everywhere",
                ["p", "g", "s", "e"].map(|t| unmergeable(|i| with_own(ids(t, i, n - 1)))),
            ),
            (
                "IDs in the person, own and IDs in the narrowings",
                [
                    unmergeable(|i| ids("p", i, n)),
                    unmergeable(|i| with_own(ids("g", i, n - 1))),
                    unmergeable(|i| with_own(ids("s", i, n - 1))),
                    unmergeable(|i| ids("e", i, n)),
                ],
            ),
            (
                "a shared pool of IDs",
                ["p", "g", "s", "e"].map(|_| unmergeable(pool)),
            ),
        ];
        for (shape, [p, g, s, e]) in shapes {
            let started = std::time::Instant::now();
            let r = effective(&p, Some(&g), Some(&s), Some(&e));
            let built = started.elapsed();
            let started = std::time::Instant::now();
            assert!(!r.allows(RightType::Comments, "read", Some("nowhere"), &own));
            let checked = started.elapsed();
            let objects = r.iter().count();
            let locations: usize = r.iter().map(|a| a.locations().len()).sum();
            eprintln!(
                "{shape}: {objects} objects, {locations} locations, built in {built:?}, a miss checked in {checked:?}"
            );
            assert!(
                objects <= 63,
                "{shape}: at most one object per comment action set: {objects}"
            );
            assert!(
                objects <= 86,
                "{shape}: at most one object per type, level and action set: {objects}"
            );
        }
    }

    /// A seeded linear congruential generator: the property tests below are
    /// deterministic and need no dependency.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    const KINDS: [RightType; 4] = [
        RightType::Artifacts,
        RightType::Comments,
        RightType::Db,
        RightType::Ask,
    ];
    const ARTIFACTS: [Option<&str>; 4] = [None, Some("a"), Some("b"), Some("c")];

    fn random_access(g: &mut Lcg) -> Access {
        let kind = KINDS[g.below(KINDS.len())];
        let all = kind.actions();
        let mut actions: Vec<&str> = all.iter().copied().filter(|_| g.below(2) == 0).collect();
        if actions.is_empty() {
            actions.push(all[g.below(all.len())]);
        }
        let universe = [
            Location::Any,
            Location::Own,
            Location::Artifact("a".into()),
            Location::Artifact("b".into()),
            Location::OwnArtifact("a".into()),
            Location::OwnArtifact("b".into()),
        ];
        let mut locations: Vec<Location> = (0..1 + g.below(2))
            .map(|_| universe[g.below(universe.len())].clone())
            .collect();
        if !kind.located() || (kind == RightType::Artifacts && actions.contains(&"create")) {
            locations = vec![Location::Any];
        }
        let level = (kind == RightType::Db).then(|| {
            let l = [Level::View, Level::Interact, Level::Admin][g.below(3)];
            if actions.contains(&"write") {
                l.max(Level::Interact)
            } else {
                l
            }
        });
        Access::new(kind, &actions, locations, level)
    }

    fn random_rights(g: &mut Lcg) -> Rights {
        Rights(merge((0..g.below(4)).map(|_| random_access(g)).collect()))
    }

    /// Calls `check(kind, action, artifact, own)` at every point of the
    /// universe, under every ownership of `a`, `b` and `c`.
    fn every_point(mut check: impl FnMut(RightType, &str, Option<&str>, &dyn Fn(&str) -> bool)) {
        for mask in 0..8u8 {
            let own = move |x: &str| match x {
                "a" => mask & 1 != 0,
                "b" => mask & 2 != 0,
                "c" => mask & 4 != 0,
                _ => false,
            };
            for kind in KINDS {
                for action in kind.actions() {
                    for artifact in ARTIFACTS {
                        check(kind, action, artifact, &own);
                    }
                }
            }
        }
    }

    #[test]
    fn the_algebra_holds_on_random_rights() {
        let mut g = Lcg(0x5eed_c1a7);
        for round in 0..600 {
            let (a, b, up) = (
                random_rights(&mut g),
                random_rights(&mut g),
                random_rights(&mut g),
            );
            let both = a.intersect(&b);
            let eff = effective(&a, Some(&b), None, Some(&up));
            let ctx = || {
                format!(
                    "round {round}\nA {}\nB {}\nE {}",
                    a.to_json(),
                    b.to_json(),
                    up.to_json()
                )
            };
            let all = [&a, &b, &up, &both, &eff];
            let narrower: Vec<(&Rights, &Rights)> = all
                .iter()
                .flat_map(|x| all.iter().map(move |y| (*x, *y)))
                .filter(|(x, y)| x.is_narrower_or_equal(y))
                .collect();
            every_point(|kind, action, artifact, own| {
                let al = |r: &Rights| r.allows(kind, action, artifact, own);
                assert_eq!(
                    al(&both),
                    al(&a) && al(&b),
                    "∩ allows, {kind:?} {action} {artifact:?}\n{}",
                    ctx()
                );
                assert_eq!(
                    al(&eff),
                    (al(&a) && al(&b)) || al(&up),
                    "effective allows\n{}",
                    ctx()
                );
                if let (RightType::Db, Some(x)) = (kind, artifact) {
                    let dl = |r: &Rights| r.db_level(x, action, own);
                    assert_eq!(
                        dl(&both),
                        dl(&a).min(dl(&b)),
                        "∩ level, {action} {x}\n{}",
                        ctx()
                    );
                    assert_eq!(
                        dl(&eff),
                        dl(&a).min(dl(&b)).max(dl(&up)),
                        "effective level\n{}",
                        ctx()
                    );
                }
                for (x, y) in &narrower {
                    assert!(!al(x) || al(y), "narrower allows more\n{}", ctx());
                    if let (RightType::Db, Some(id)) = (kind, artifact) {
                        assert!(
                            x.db_level(id, action, own) <= y.db_level(id, action, own),
                            "narrower level\n{}",
                            ctx()
                        );
                    }
                }
            });
            let all_own = |_: &str| true;
            for kind in KINDS {
                for action in kind.actions() {
                    let anywhere = ARTIFACTS
                        .iter()
                        .any(|x| a.allows(kind, action, *x, &all_own));
                    assert_eq!(
                        a.allows_somewhere(kind, action),
                        anywhere,
                        "somewhere\n{}",
                        ctx()
                    );
                }
            }
            assert!(
                both.is_narrower_or_equal(&a) && both.is_narrower_or_equal(&b),
                "∩ narrower\n{}",
                ctx()
            );
            assert!(
                a.is_narrower_or_equal(&eff.union(&a)),
                "reflexive under ∪\n{}",
                ctx()
            );
            for r in [&a, &both, &eff] {
                assert_eq!(
                    &Rights::parse_internal(&r.to_json()).unwrap(),
                    r,
                    "round trip\n{}",
                    ctx()
                );
            }
        }
    }
}

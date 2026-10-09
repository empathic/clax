//! Access rights (spec §7): GNAP (RFC 9635) access objects with Clax types,
//! actions and locations, the reference presets, and the effective rights of
//! an agent. Evaluation is pure; `own` (artifacts the agent's person created)
//! is resolved by the caller at check time through a predicate.

use crate::db::Level;
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

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

/// Most access objects in one rights array.
pub const MAX_ACCESS: usize = 64;
/// Most locations in one access object.
pub const MAX_LOCATIONS: usize = 64;
/// Longest artifact ID accepted as a location, in bytes.
pub const MAX_LOCATION_BYTES: usize = 64;

const INVALID: &str = "invalid_rights";
/// The wire prefix of [`Location::OwnArtifact`].
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

/// Where an access object applies.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Location {
    /// `"*"`: every artifact.
    Any,
    /// `"own"`: artifacts the agent's person created, resolved at check time.
    Own,
    /// An artifact ID.
    Artifact(String),
    /// `"own:<id>"`: the artifact, only while it is the person's own. It is
    /// what `own ∩ <id>` leaves; ownership is still resolved at check time.
    OwnArtifact(String),
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_LOCATION_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Location {
    pub fn parse(s: &str) -> Result<Location> {
        match s {
            "*" => Ok(Location::Any),
            "own" => Ok(Location::Own),
            _ => {
                if let Some(id) = s.strip_prefix(OWN_PREFIX)
                    && valid_id(id)
                {
                    return Ok(Location::OwnArtifact(id.to_string()));
                }
                if valid_id(s) {
                    Ok(Location::Artifact(s.to_string()))
                } else {
                    Err(invalid(format!(
                        "location '{s}' is not \"*\", \"own\" or an artifact ID"
                    )))
                }
            }
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

    /// Whether this location covers `artifact` (`None`: no artifact, as for
    /// `create`, which only `*` covers).
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
/// for `clax-ask`); `level` is set exactly for `clax-db`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub kind: RightType,
    pub actions: BTreeSet<String>,
    pub locations: Vec<Location>,
    pub level: Option<Level>,
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

    fn parse(v: &Value) -> Result<Access> {
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
            Some(Value::Array(ls)) if !ls.is_empty() && ls.len() <= MAX_LOCATIONS => ls
                .iter()
                .map(|l| {
                    l.as_str()
                        .ok_or_else(|| invalid("a location must be a string"))
                        .and_then(Location::parse)
                })
                .collect::<Result<Vec<Location>>>()?,
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
        let level = match (kind, obj.get("level")) {
            (RightType::Db, None) => Some(if actions.contains("write") {
                Level::Interact
            } else {
                Level::View
            }),
            (RightType::Db, Some(l)) => match l.as_str().and_then(Level::parse) {
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
        let locations: Vec<Location> = self
            .locations
            .iter()
            .flat_map(|a| other.locations.iter().filter_map(move |b| a.and(b)))
            .collect();
        if actions.is_empty() || locations.is_empty() {
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

/// Sorted, deduplicated, and `[Any]` when any location is `*`.
fn canonical_locations(mut ls: Vec<Location>) -> Vec<Location> {
    if ls.contains(&Location::Any) {
        return vec![Location::Any];
    }
    ls.sort();
    ls.dedup();
    ls
}

/// A rights array: what is allowed is what any one access object allows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rights(pub Vec<Access>);

impl Rights {
    /// Parses a GNAP access array whose items are access objects or preset
    /// names, or a single preset name. Anything outside the vocabulary is 400
    /// `invalid_rights`.
    pub fn parse(v: &Value) -> Result<Rights> {
        let items = match v {
            Value::String(name) => return preset_or_invalid(name),
            Value::Array(items) => items,
            _ => return Err(invalid("rights must be an array of access objects")),
        };
        let mut out = Vec::new();
        for item in items {
            match item {
                Value::String(name) => out.extend(preset_or_invalid(name)?.0),
                _ => out.push(Access::parse(item)?),
            }
            if out.len() > MAX_ACCESS {
                return Err(invalid(format!("at most {MAX_ACCESS} access objects")));
            }
        }
        Ok(Rights(dedup(out)))
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
        Some(Rights(rights))
    }

    /// `clax:full`, the owner's rights.
    pub fn full() -> Rights {
        Rights::preset(PRESET_FULL).unwrap_or_default()
    }

    /// What both allow: an action at a location only where both allow it,
    /// `level` the lower.
    pub fn intersect(&self, other: &Rights) -> Rights {
        let out = self
            .0
            .iter()
            .flat_map(|a| other.0.iter().filter_map(move |b| a.and(b)))
            .collect();
        Rights(dedup(out))
    }

    /// What either allows.
    pub fn union(&self, other: &Rights) -> Rights {
        Rights(dedup(self.0.iter().chain(&other.0).cloned().collect()))
    }

    /// Whether `other` allows everything this does, at least at this level,
    /// whoever owns what.
    pub fn is_narrower_or_equal(&self, other: &Rights) -> bool {
        self.0.iter().all(|a| {
            a.locations.iter().all(|l| {
                let at: Vec<&Access> = other
                    .0
                    .iter()
                    .filter(|b| b.kind == a.kind && b.locations.iter().any(|m| l.within(m)))
                    .collect();
                a.actions
                    .iter()
                    .all(|act| at.iter().any(|b| b.actions.contains(act)))
                    && at.iter().filter_map(|b| b.level).max() >= a.level
            })
        })
    }

    /// Whether these rights allow `action` of `kind` at `artifact` (`None`
    /// for an action at no artifact, such as `create` or `ask`). `own` says
    /// whether the agent's person created an artifact.
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

    /// The db level these rights give at `artifact`: the highest of the
    /// `clax-db` objects covering it, or `None` without db access there.
    pub fn db_level(&self, artifact: &str, own: &dyn Fn(&str) -> bool) -> Option<Level> {
        self.0
            .iter()
            .filter(|a| a.kind == RightType::Db && a.covers(Some(artifact), own))
            .filter_map(|a| a.level)
            .max()
    }

    /// The GNAP access array, as [`Rights::parse`] reads it back.
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

fn dedup(items: Vec<Access>) -> Vec<Access> {
    let mut out: Vec<Access> = Vec::with_capacity(items.len());
    for a in items {
        if !out.contains(&a) {
            out.push(a);
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
        assert_eq!(c.db_level("other", &own), Some(Level::Interact));
        assert_eq!(c.db_level("mine", &own), Some(Level::Admin));
        let r = Rights::preset(PRESET_READER).unwrap();
        assert!(!r.allows(RightType::Comments, "resolve", Some("x"), &own));
        assert!(!r.allows(RightType::Comments, "write", Some("x"), &own));
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
            e.db_level("other", &own),
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
        assert_eq!(Rights::parse(&e.to_json()).unwrap(), e);
    }

    #[test]
    fn missing_narrowings_narrow_nothing_and_ask_needs_no_artifact() {
        let person = Rights::preset(PRESET_READER).unwrap();
        assert_eq!(effective(&person, None, None, None), person);
        assert!(person.allows(RightType::Ask, "ask", None, &own));
        assert_eq!(person.db_level("x", &own), Some(Level::View));
        assert_eq!(Rights::parse(&json!([])).unwrap().db_level("x", &own), None);
        for bad in [
            json!([{"type": "clax-ask", "actions": ["ask"], "locations": ["x"]}]),
            json!([{"type": "clax-comments", "actions": ["read"], "level": "view"}]),
            json!([{"type": "clax-db", "actions": []}]),
            json!([{"type": "clax-db", "actions": ["read"], "locations": ["a b"]}]),
            json!([{"type": "clax-db", "actions": ["read"], "privileges": ["x"]}]),
            json!({"type": "clax-db", "actions": ["read"]}),
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
}

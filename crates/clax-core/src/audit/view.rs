//! The audit view's answers and query (`GET /api/audit`, `GET
//! /api/audit/<seq>`; spec 2026-10-21-claxbot-design §9.2), as the API
//! contract (`crate::contract`) has them. The route serializes these; the
//! CLI and the MCP tool read them back.

use super::KindGroup;
use serde::{Deserialize, Serialize};

/// Who acted, as the view names them: a person key (`p_…`, or `*owner`),
/// `agent:claxbot`, another agent's handle, or `*system`, with its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuditActor {
    pub id: String,
    pub name: Option<String>,
}

/// A recorded event, newest first in a page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuditEvent {
    /// Its place in the journal; larger is later.
    pub seq: i64,
    /// When, RFC 3339.
    pub at: String,
    pub actor: AuditActor,
    /// The person ClaxBot acted for, for a ClaxBot event.
    pub on_behalf_of: Option<AuditActor>,
    /// What happened: `artifact.publish`, `view.audit`, `bot.call`, ….
    pub kind: String,
    /// The event in one sentence.
    pub summary: String,
    pub artifact_id: Option<String>,
    /// The ClaxBot run it belongs to.
    pub run: Option<String>,
}

/// An event with its stored body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuditEventDetail {
    #[serde(flatten)]
    pub event: AuditEvent,
    /// The body as recorded (its envelope included); its shape is the
    /// kind's.
    pub body: serde_json::Value,
}

/// A page of events, newest first; `next_cursor` is the `seq` the next
/// (older) page starts below, `null` on the last.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuditPage {
    pub events: Vec<AuditEvent>,
    pub next_cursor: Option<i64>,
}

/// `GET /api/audit`'s query. Each parameter at most once; an empty one is
/// none; any other is 400 `invalid_parameter`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuditListQuery {
    /// Words found in the kind, the actor or the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// `claxbot`, or a person key (that person and their agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<KindGroup>,
    /// One kind (`artifact.publish`), or the kinds under a prefix
    /// (`bot.*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Events naming this artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// From this time, inclusive: RFC 3339, or `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Until this time, exclusive: RFC 3339, or `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// A page's `next_cursor`: events before that `seq`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<i64>,
    /// At most this many, 1 to 200 (by default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl AuditListQuery {
    /// The query's parameter names, in the order of its fields.
    pub const NAMES: &'static [&'static str] = &[
        "q", "actor", "group", "kind", "artifact", "from", "to", "cursor", "limit",
    ];

    /// The query string (without `?`) of the parameters given.
    #[must_use]
    pub fn to_query(&self) -> String {
        let mut s = url::form_urlencoded::Serializer::new(String::new());
        let pairs: [(&str, Option<String>); 9] = [
            ("q", self.q.clone()),
            ("actor", self.actor.clone()),
            ("group", self.group.map(|g| g.as_str().to_string())),
            ("kind", self.kind.clone()),
            ("artifact", self.artifact.clone()),
            ("from", self.from.clone()),
            ("to", self.to.clone()),
            ("cursor", self.cursor.map(|c| c.to_string())),
            ("limit", self.limit.map(|l| l.to_string())),
        ];
        for (k, v) in pairs {
            if let Some(v) = v {
                s.append_pair(k, &v);
            }
        }
        s.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_are_the_fields() {
        let s = schemars::schema_for!(AuditListQuery).to_value();
        let mut fields: Vec<&str> = s["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut names = AuditListQuery::NAMES.to_vec();
        fields.sort_unstable();
        names.sort_unstable();
        assert_eq!(fields, names);
    }

    #[test]
    fn a_query_string_holds_the_parameters_given() {
        let q = AuditListQuery {
            group: Some(KindGroup::Bot),
            kind: Some("bot.*".into()),
            from: Some("2026-11-01".into()),
            limit: Some(5),
            ..Default::default()
        };
        assert_eq!(q.to_query(), "group=bot&kind=bot.*&from=2026-11-01&limit=5");
        assert_eq!(AuditListQuery::default().to_query(), "");
    }

    #[test]
    fn a_detail_is_an_event_and_its_body() {
        let d = AuditEventDetail {
            event: AuditEvent {
                seq: 7,
                at: "2026-11-02T00:00:00Z".into(),
                actor: AuditActor {
                    id: "*owner".into(),
                    name: None,
                },
                on_behalf_of: None,
                kind: "view.audit".into(),
                summary: "Viewed the audit".into(),
                artifact_id: None,
                run: None,
            },
            body: serde_json::json!({"filters": ""}),
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["seq"], 7);
        assert_eq!(v["body"]["filters"], "");
        assert_eq!(serde_json::from_value::<AuditEventDetail>(v).unwrap(), d);
    }
}

//! The JSON view of an artifact the routes and the shell's first-load data
//! share: its owner fields, its live page, and the session IDs a token-less
//! caller must not see.

use clax_core::live::KIND_LIVE;
use clax_core::model::{Artifact, Session};
use clax_core::store::live::LivePage;
use clax_core::{ArtifactId, Participants, Store};
use serde_json::{Value, json};

/// `a` as JSON with `owner_live` (its owner session exists and has not ended),
/// `owner_harness` (the owner's harness, when it exists), `owner_name` (its
/// agent name, when it has one) and `participants`
/// (people by public ID, agents by handle; see [`Store::participants`]).
/// A live page (`kind` `live`) also carries `live: {origin, path, page_url,
/// origins}` from `live`: `origins` are its site's (spec §7.2), the most
/// recently used first, and `page_url` is the page's path on the first.
/// `working` is the artifact's working list (spec §10 "Working"), which
/// never names a session. The
/// artifact's own `owner_session_id` stays; token-less routes drop it with
/// [`strip_sessions`].
pub(crate) fn with_owner(
    a: &Artifact,
    owner: Option<&Session>,
    working: &[clax_core::working::WorkingView],
    participants: &Participants,
    live: Option<&LivePart>,
) -> Value {
    let mut v = serde_json::to_value(a).expect("serialisable artifact");
    v["owner_live"] = json!(owner.is_some_and(|o| o.ended_at.is_none()));
    v["owner_harness"] = json!(owner.map(|o| &o.harness));
    v["owner_name"] = json!(owner.and_then(|o| o.name.as_ref()));
    v["working"] = json!(working);
    v["participants"] = json!(participants);
    if let Some(LivePart {
        page: p,
        origins,
        merged_into,
    }) = live
    {
        let newest = origins.first().unwrap_or(&p.origin);
        v["live"] = json!({
            "origin": p.origin,
            "path": p.path,
            "page_url": format!("{newest}{}", p.path),
            "origins": origins,
        });
        if let Some((m, url)) = merged_into {
            v["live"]["merged_into"] = json!(m);
            v["live"]["merged_into_url"] = json!(url);
        }
    }
    v
}

/// A live page with its site's origins, the most recently used first.
pub(crate) struct LivePart {
    pub page: LivePage,
    pub origins: Vec<String>,
    /// A page a join merged away (spec §7.2): the page it was merged into,
    /// and that page's URL.
    pub merged_into: Option<(String, Option<String>)>,
}

/// The live page `a` is, when it is one.
pub(crate) fn live_part(st: &Store, a: &Artifact) -> clax_core::Result<Option<LivePart>> {
    if a.kind != KIND_LIVE {
        return Ok(None);
    }
    let Some(page) = st.live_page_of(&ArtifactId::parse(&a.id)?)? else {
        let Some((origin, path, into)) = st.merged_live_page(&a.id)? else {
            return Ok(None);
        };
        let page = LivePage {
            artifact_id: a.id.clone(),
            origin: origin.clone(),
            path,
        };
        return Ok(Some(LivePart {
            page,
            origins: vec![origin],
            merged_into: match into {
                Some(i) => {
                    let url = st
                        .live_page_of(&ArtifactId::parse(&i)?)?
                        .map(|p| format!("{}{}", p.origin, p.path));
                    Some((i, url))
                }
                None => None,
            },
        }));
    };
    let origins = st.joined_site(&page.origin)?.origin_names();
    Ok(Some(LivePart {
        page,
        origins,
        merged_into: None,
    }))
}

/// Leaves out the session IDs a token-less caller must not see (spec §14):
/// the artifact's `owner_session_id` and each version's `session_id`.
pub(crate) fn strip_sessions(v: &mut Value) {
    if let Some(a) = v.get_mut("artifact").and_then(Value::as_object_mut) {
        a.remove("owner_session_id");
    }
    if let Some(x) = v.get_mut("version").and_then(Value::as_object_mut) {
        x.remove("session_id");
    }
    for x in v
        .get_mut("versions")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(x) = x.as_object_mut() {
            x.remove("session_id");
        }
    }
    for x in v
        .get_mut("artifacts")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(x) = x.as_object_mut() {
            x.remove("owner_session_id");
        }
    }
}

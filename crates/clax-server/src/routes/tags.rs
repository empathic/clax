//! Tags ([`clax_core::tags`], [`clax_store::tags`]): one list the owner
//! manages, and the tags each artifact carries.
//!
//! - `GET /api/tags`: the list, with how many artifacts the caller can see
//!   carry each.
//! - `POST /api/tags`, `PATCH /api/tags/<tag_id>`, `DELETE
//!   /api/tags/<tag_id>`: the owner's (their browser, by hand, and their
//!   agents) alone.
//! - `PUT /api/artifacts/<aid>/tags {tags}` sets an artifact's tags and
//!   `PATCH /api/artifacts/<aid>/tags {add, remove}` changes them: those
//!   who administer the artifact ([`clax_http::agent::administers`]) alone,
//!   as for its pin. Only existing tags are applied: an unknown one is
//!   refused, naming the tags there are.
//!
//! Each change is recorded ([`clax_store::tags`]) and announced on the
//! stream ([`Event::Tags`]).

use super::artifacts::{body, path};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use clax_core::Event;
use clax_core::audit::AuditCtx;
use clax_core::live::KIND_LIVE;
use clax_core::tags::{COLOURS, Tag};
use clax_http::agent::{Agent, person_agent_session};
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::tags::{NewTag, TagEdit, TagPatch};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Whether `c` manages the tag list: the owner (browser or by hand) and
/// the owner's agents, a grant of the owner's row included.
fn manages_tags(c: &Caller) -> bool {
    matches!(
        c,
        Caller::Owner { .. } | Caller::OwnerAgent { .. } | Caller::PersonAgent { person: None, .. }
    )
}

/// 401 without a credential, else 403 `owner_only`, unless `c` manages the
/// tag list ([`manages_tags`]).
fn require_manager(c: &Caller) -> Result<(), ApiError> {
    match c {
        _ if manages_tags(c) => Ok(()),
        Caller::Anonymous => Err(ApiError::unauthorized()),
        _ => Err(ApiError::forbidden(
            "owner_only",
            "only the owner and the owner's agents make, rename, recolour or delete tags",
        )),
    }
}

/// `tag` as the list serves it, with `count`.
fn tag_view(tag: &Tag, count: usize) -> Value {
    let mut v = json!(tag);
    v["count"] = json!(count);
    v
}

/// `GET /api/tags` → `{tags, colours, may_manage}`: each tag (`{id, name,
/// colour, description, created_at, updated_at, count}`, sorted by name),
/// `count` the artifacts the caller can see that carry it; the palette
/// keys a tag's colour is one of; and whether the caller makes and changes
/// tags (the owner and the owner's agents). The owner, people, members of
/// a hosted Clax and agents see every tag, so they know what they may
/// apply; a guest (a viewer who is no connected person, or no credential)
/// sees only the tags of the artifacts it can see. Live pages count only
/// for a caller who sees them ([`clax_http::live_access::sees_live_pages`]).
pub async fn list(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::TagsList>,
    extensions: axum::http::Extensions,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let local = clax_http::live_access::sees_live_pages(&headers, &extensions, &s.token);
    let reads = a.read_rights();
    let guest = matches!(a.caller, Caller::Viewer { .. } | Caller::Anonymous);
    let may_manage = manages_tags(&a.caller);
    let out = s
        .store_call_bulk(move |st| {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for art in st.list_artifacts()? {
                if (!local && art.kind == KIND_LIVE) || !reads.allows(&art) {
                    continue;
                }
                for t in &art.tags {
                    *counts.entry(t.id.clone()).or_default() += 1;
                }
            }
            let tags: Vec<Value> = st
                .list_tags()?
                .iter()
                .map(|t| (t, counts.get(&t.id).copied().unwrap_or(0)))
                .filter(|(_, n)| !guest || *n > 0)
                .map(|(t, n)| tag_view(t, n))
                .collect();
            Ok(json!({"tags": tags, "colours": COLOURS, "may_manage": may_manage}))
        })
        .await?;
    Ok(Json(out))
}

/// `POST /api/tags` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    name: String,
    colour: Option<String>,
    description: Option<String>,
}

/// `POST /api/tags {name, colour?, description?}` → 201 `{tag}`: makes a
/// tag (colour `gray` when none is named). The owner and the owner's agents
/// alone (403 `owner_only`; 401 without a credential). 400 `invalid_tag`
/// (a name blank, over 40 characters after trimming, holding a control or
/// hidden character, or shaped like a tag ID; a description over 200
/// characters or holding one) or `invalid_colour` (not a palette key); 409
/// `tag_exists` (a tag has that name, whatever its case); 429
/// `limit_reached` past 500 tags.
pub async fn create(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::TagsCreate>,
    _o: clax_http::viewer::SameOrigin,
    audit: AuditCtx,
    req: Result<Json<CreateBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    require_manager(&a.caller)?;
    let b = body(req)?;
    let events = s.events.clone();
    let tag = s
        .store_call(move |st| {
            let tag = st.create_tag(
                &audit,
                NewTag {
                    name: b.name,
                    colour: b.colour,
                    description: b.description,
                },
            )?;
            events.publish(Event::Tags {
                artifact_id: None,
                carriers: vec![],
            });
            Ok(tag)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(json!({"tag": tag_view(&tag, 0)}))))
}

/// `PATCH /api/tags/<tag_id>` body: each field given is set (a blank
/// `description` clears it).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateBody {
    name: Option<String>,
    colour: Option<String>,
    description: Option<String>,
}

/// `PATCH /api/tags/<tag_id> {name?, colour?, description?}` → `{tag}`
/// (without `count`): renames, recolours or redescribes a tag; a rename
/// keeps its ID, and every artifact carrying it carries the new name. The
/// owner and the owner's agents alone; 404 for no such tag; the values'
/// errors as for [`create`].
pub async fn update(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::TagsUpdate>,
    _o: clax_http::viewer::SameOrigin,
    audit: AuditCtx,
    id: Result<Path<String>, PathRejection>,
    req: Result<Json<UpdateBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    require_manager(&a.caller)?;
    let id = path(id)?;
    let b = body(req)?;
    let events = s.events.clone();
    let tag = s
        .store_call(move |st| {
            let before = st.get_tag(&id)?;
            let tag = st.update_tag(
                &audit,
                &id,
                TagPatch {
                    name: b.name,
                    colour: b.colour,
                    description: b.description,
                },
            )?;
            if before.as_ref() != Some(&tag) {
                events.publish(Event::Tags {
                    artifact_id: None,
                    carriers: st.tag_carriers(&tag.id)?,
                });
            }
            Ok(tag)
        })
        .await?;
    Ok(Json(json!({"tag": tag})))
}

/// `DELETE /api/tags/<tag_id>` → `{deleted: true, artifacts}`: deletes a
/// tag and removes it from every artifact (`artifacts`: how many carried
/// it). The owner and the owner's agents alone; 404 for no such tag.
pub async fn delete(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::TagsDelete>,
    _o: clax_http::viewer::SameOrigin,
    audit: AuditCtx,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    require_manager(&a.caller)?;
    let id = path(id)?;
    let events = s.events.clone();
    let carried = s
        .store_call(move |st| {
            let carriers = st.delete_tag(&audit, &id)?;
            let carried = carriers.len();
            events.publish(Event::Tags {
                artifact_id: None,
                carriers,
            });
            Ok(carried)
        })
        .await?;
    Ok(Json(json!({"deleted": true, "artifacts": carried})))
}

/// `PUT /api/artifacts/<aid>/tags` body: the tags it carries after, each by
/// ID or name.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBody {
    tags: Vec<String>,
}

/// `PATCH /api/artifacts/<aid>/tags` body: tags to add, then tags to
/// remove, each by ID or name.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

/// Applies `edit` to the tags of the artifact `a`'s path names, as
/// [`put_artifact`] says.
async fn edit_artifact(
    s: &AppState,
    caller: &Caller,
    aid: clax_core::ArtifactId,
    audit: AuditCtx,
    edit: TagEdit,
) -> Result<Json<Value>, ApiError> {
    person_agent_session(caller)?;
    if matches!(caller, Caller::Anonymous) {
        return Err(ApiError::unauthorized());
    }
    let c = caller.clone();
    let events = s.events.clone();
    let tags = s
        .store_call(move |st| {
            // No such artifact is the edit's own answer.
            if st.get_artifact(&aid)?.is_none() {
                return Err(clax_core::CoreError::NotFound);
            }
            if !clax_http::agent::administers(st, &c, aid.as_str())? {
                return Ok(None);
            }
            let (after, changed) = st.edit_artifact_tags(&audit, &aid, &edit)?;
            if changed {
                events.publish(Event::Tags {
                    artifact_id: Some(aid.as_str().to_string()),
                    carriers: vec![],
                });
            }
            Ok(Some((aid.as_str().to_string(), after.tags, changed)))
        })
        .await?;
    let Some((aid, tags, changed)) = tags else {
        return Err(ApiError::forbidden(
            "admin_only",
            "only the owner, the page's maker and their agents change its tags",
        ));
    };
    Ok(Json(
        json!({"artifact_id": aid, "tags": tags, "changed": changed}),
    ))
}

/// `PUT /api/artifacts/<aid>/tags {tags: [id or name, …]}` → `{artifact_id,
/// tags, changed}`: the artifact carries exactly these existing tags
/// (`[]` clears them); `tags` the tags it carries after (`{id, name,
/// colour}`, sorted by name), `changed` whether that differs from before.
/// Those who administer the artifact alone (the owner and the owner's
/// agents, and the person who made it and that person's agents): 403
/// `admin_only` for anyone else (an editor included), 401 without a
/// credential; a person's agent writes from a session (400 `no_session`).
/// 400 `unknown_tag` for a tag that names none, its message listing the
/// tags there are (agents never make tags), `too_many_tags` past 20; 404
/// for no such artifact.
pub async fn put_artifact(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ArtifactTagsPut>,
    _o: clax_http::viewer::SameOrigin,
    audit: AuditCtx,
    req: Result<Json<PutBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let aid = a.artifact()?;
    let b = body(req)?;
    edit_artifact(&s, &a.caller, aid, audit, TagEdit::Replace(b.tags)).await
}

/// `PATCH /api/artifacts/<aid>/tags {add?: [id or name, …], remove?: […]}`
/// → as [`put_artifact`]: adds tags (one it carries already stays), then
/// removes tags (one it does not carry is no change). Who may, and the
/// errors, as for [`put_artifact`].
pub async fn patch_artifact(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::ArtifactTagsPatch>,
    _o: clax_http::viewer::SameOrigin,
    audit: AuditCtx,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let aid = a.artifact()?;
    let b = body(req)?;
    edit_artifact(
        &s,
        &a.caller,
        aid,
        audit,
        TagEdit::Change {
            add: b.add,
            remove: b.remove,
        },
    )
    .await
}

/// The `tag` parameters of a query string (`?tag=a&tag=b`), each trimmed,
/// blanks left out.
pub(crate) fn tag_params(query: Option<&str>) -> Vec<String> {
    url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .filter(|(k, _)| k == "tag")
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_params_are_repeated_and_decoded() {
        assert_eq!(
            tag_params(Some("tag=a&x=1&tag=b%20c&tag=&tag=+d+")),
            ["a", "b c", "d"]
        );
        assert!(tag_params(None).is_empty());
    }
}

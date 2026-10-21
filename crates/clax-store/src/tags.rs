//! Tags (migration 41, [`clax_core::tags`]): one list the owner manages
//! (`tags`) and the tags each artifact carries (`artifact_tags`). A tag's
//! name is unique as [`clax_core::tags::fold`] folds it; a rename keeps the
//! tag's ID, so the artifacts carrying it carry the new name. Deleting a tag
//! removes it from every artifact. Each change is recorded: `tag.create`,
//! `tag.update` and `tag.delete` for the list, `artifact.update` (its
//! `fields.tags`, the names after the change) for an artifact's tags.
//!
//! Who may change what is the routes' to decide; the store checks only the
//! values and the bounds ([`MAX_TAGS`], [`MAX_ARTIFACT_TAGS`]).

use super::Store;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::model::Artifact;
use clax_core::tags::{
    self, DEFAULT_COLOUR, MAX_ARTIFACT_TAGS, MAX_TAGS, Tag, check_colour, check_description,
    check_name, fold,
};
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{OptionalExtension, Row, params};

const SELECT: &str = "SELECT id, name, colour, description, created_at, updated_at FROM tags";

fn row_to_tag(r: &Row<'_>) -> rusqlite::Result<Tag> {
    Ok(Tag {
        id: r.get(0)?,
        name: r.get(1)?,
        colour: r.get(2)?,
        description: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

/// A new tag's values, unchecked.
#[derive(Clone, Debug, Default)]
pub struct NewTag {
    pub name: String,
    /// [`DEFAULT_COLOUR`] when `None`.
    pub colour: Option<String>,
    pub description: Option<String>,
}

/// A change to a tag: each field that is `Some` is set (a blank
/// `description` clears it).
#[derive(Clone, Debug, Default)]
pub struct TagPatch {
    pub name: Option<String>,
    pub colour: Option<String>,
    pub description: Option<String>,
}

/// A change to the tags an artifact carries, each tag named by ID or name
/// ([`Store::resolve_tags`]).
#[derive(Clone, Debug)]
pub enum TagEdit {
    /// The artifact carries exactly these.
    Replace(Vec<String>),
    /// These are added (those it carries already stay) and then those are
    /// removed (those it does not carry are no change).
    Change {
        add: Vec<String>,
        remove: Vec<String>,
    },
}

/// 409 `tag_exists`: another tag has `name`, as names are compared.
fn exists(name: &str) -> CoreError {
    CoreError::invalid("tag_exists", format!("a tag named '{name}' exists"))
}

/// The names of `all`, for an error that lists them.
fn names_list(all: &[Tag]) -> String {
    if all.is_empty() {
        "there are no tags yet; the owner makes them (clax tags create)".to_string()
    } else {
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        format!("the tags are: {}", names.join(", "))
    }
}

/// The live artifacts on `c` that carry tag `id`, by ID.
fn carriers_in(c: &rusqlite::Connection, id: &str) -> Result<Vec<String>> {
    let mut st = c.prepare_cached(
        "SELECT x.artifact_id FROM artifact_tags x JOIN artifacts a ON a.id = x.artifact_id
          WHERE x.tag_id = ?1 AND a.deleted_at IS NULL ORDER BY x.artifact_id",
    )?;
    let rows = st.query_map(params![id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Every tag on `c`, sorted by name as names are compared.
fn all_tags(c: &rusqlite::Connection) -> Result<Vec<Tag>> {
    let mut st = c.prepare_cached(&format!("{SELECT} ORDER BY fold, id"))?;
    let rows = st.query_map([], row_to_tag)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Each of `refs` (an ID, or a name as names are compared) as the tag it
/// names in `all`, in order and without repeats.
///
/// # Errors
/// `unknown_tag` naming the first that names none and listing the tags.
fn resolve_in(all: &[Tag], refs: &[String]) -> Result<Vec<Tag>> {
    let mut out: Vec<Tag> = Vec::new();
    for r in refs {
        let key = fold(r.trim());
        let found = all
            .iter()
            .find(|t| t.id == r.trim())
            .or_else(|| all.iter().find(|t| fold(&t.name) == key));
        match found {
            Some(t) if out.iter().any(|o| o.id == t.id) => {}
            Some(t) => out.push(t.clone()),
            None => {
                return Err(CoreError::invalid(
                    "unknown_tag",
                    format!(
                        "no tag is named '{}': {}. Only the owner makes tags",
                        r.trim(),
                        names_list(all)
                    ),
                ));
            }
        }
    }
    Ok(out)
}

impl Store {
    /// Every tag, sorted by name as names are compared.
    ///
    /// # Errors
    /// The store's.
    pub fn list_tags(&self) -> Result<Vec<Tag>> {
        self.with_read(all_tags)
    }

    /// Tag `id`, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn get_tag(&self, id: &str) -> Result<Option<Tag>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(&format!("{SELECT} WHERE id = ?1"))?
                .query_row(params![id], row_to_tag)
                .optional()?)
        })
    }

    /// The tags `refs` name, each by ID or by name as names are compared, in
    /// order and without repeats.
    ///
    /// # Errors
    /// `unknown_tag` for one that names no tag, its message listing the
    /// tags; the store's.
    pub fn resolve_tags(&self, refs: &[String]) -> Result<Vec<Tag>> {
        self.with_read(|c| resolve_in(&all_tags(c)?, refs))
    }

    /// Makes a tag, recorded as `tag.create` under `ctx`.
    ///
    /// # Errors
    /// `invalid_tag` or `invalid_colour` for a value
    /// ([`clax_core::tags`]); `tag_exists` for a name another tag has;
    /// `limit_reached` past [`MAX_TAGS`].
    pub fn create_tag(&self, ctx: &AuditCtx, new: NewTag) -> Result<Tag> {
        let name = check_name(&new.name)?;
        let colour = check_colour(new.colour.as_deref().unwrap_or(DEFAULT_COLOUR))?;
        let description = match &new.description {
            Some(d) => check_description(d)?,
            None => None,
        };
        self.with_tx(|tx| {
            let n: usize = tx.query_row("SELECT count(*) FROM tags", [], |r| r.get(0))?;
            if n >= MAX_TAGS {
                return Err(CoreError::invalid(
                    "limit_reached",
                    format!("a Clax keeps at most {MAX_TAGS} tags"),
                ));
            }
            let now = self.now();
            let tag = Tag {
                id: tags::new_id(),
                name: name.clone(),
                colour: colour.clone(),
                description: description.clone(),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            let made = tx.execute(
                "INSERT OR IGNORE INTO tags (id, name, fold, colour, description, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![tag.id, tag.name, fold(&tag.name), tag.colour, tag.description, now],
            )?;
            if made == 0 {
                return Err(exists(&name));
            }
            let rec = AuditRecord::new(AuditKind::TagCreate, now)
                .with("tag_id", tag.id.as_str())
                .with("name", tag.name.as_str())
                .with("colour", tag.colour.as_str())
                .with("description", tag.description.clone());
            self.record_audit(tx, ctx, rec)?;
            Ok(tag)
        })
    }

    /// Changes tag `id` as `patch` says, recorded as `tag.update` under
    /// `ctx` with the fields it set (`fields`) when it changes anything.
    /// A rename keeps the ID.
    ///
    /// # Errors
    /// `NotFound` for no such tag; [`Store::create_tag`]'s for a value.
    pub fn update_tag(&self, ctx: &AuditCtx, id: &str, patch: TagPatch) -> Result<Tag> {
        let name = patch.name.as_deref().map(check_name).transpose()?;
        let colour = patch.colour.as_deref().map(check_colour).transpose()?;
        let description = patch
            .description
            .as_deref()
            .map(check_description)
            .transpose()?;
        self.with_tx(|tx| {
            let before = tx
                .prepare_cached(&format!("{SELECT} WHERE id = ?1"))?
                .query_row(params![id], row_to_tag)
                .optional()?
                .ok_or(CoreError::NotFound)?;
            let mut after = before.clone();
            let mut fields = serde_json::Map::new();
            if let Some(n) = &name
                && *n != before.name
            {
                after.name = n.clone();
                fields.insert("name".into(), n.as_str().into());
            }
            if let Some(c) = &colour
                && *c != before.colour
            {
                after.colour = c.clone();
                fields.insert("colour".into(), c.as_str().into());
            }
            if let Some(d) = &description
                && *d != before.description
            {
                after.description = d.clone();
                fields.insert("description".into(), d.clone().into());
            }
            if fields.is_empty() {
                return Ok(before);
            }
            let now = self.now();
            after.updated_at = now.clone();
            let clash: Option<String> = tx
                .query_row(
                    "SELECT id FROM tags WHERE fold = ?1 AND id != ?2",
                    params![fold(&after.name), id],
                    |r| r.get(0),
                )
                .optional()?;
            if clash.is_some() {
                return Err(exists(&after.name));
            }
            tx.execute(
                "UPDATE tags SET name = ?2, fold = ?3, colour = ?4, description = ?5, updated_at = ?6
                  WHERE id = ?1",
                params![
                    id,
                    after.name,
                    fold(&after.name),
                    after.colour,
                    after.description,
                    now
                ],
            )?;
            let rec = AuditRecord::new(AuditKind::TagUpdate, now)
                .with("tag_id", id)
                .with("fields", fields);
            self.record_audit(tx, ctx, rec)?;
            Ok(after)
        })
    }

    /// The live artifacts that carry tag `id`, by ID.
    ///
    /// # Errors
    /// The store's.
    pub fn tag_carriers(&self, id: &str) -> Result<Vec<String>> {
        self.with_read(|c| carriers_in(c, id))
    }

    /// Deletes tag `id` and removes it from every artifact, recorded as
    /// `tag.delete` under `ctx` with its name and how many live artifacts
    /// carried it (`artifacts`). Returns those artifacts' IDs.
    ///
    /// # Errors
    /// `NotFound` for no such tag.
    pub fn delete_tag(&self, ctx: &AuditCtx, id: &str) -> Result<Vec<String>> {
        self.with_tx(|tx| {
            let name: String = tx
                .query_row("SELECT name FROM tags WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .optional()?
                .ok_or(CoreError::NotFound)?;
            let carried = carriers_in(tx, id)?;
            tx.execute("DELETE FROM artifact_tags WHERE tag_id = ?1", params![id])?;
            tx.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
            let rec = AuditRecord::new(AuditKind::TagDelete, self.now())
                .with("tag_id", id)
                .with("name", name)
                .with("artifacts", carried.len());
            self.record_audit(tx, ctx, rec)?;
            Ok(carried)
        })
    }

    /// Changes the tags live artifact `aid` carries as `edit` says. When
    /// the set changes, recorded as `artifact.update` under `ctx`, its
    /// `fields.tags` the names it carries after; `changed` says whether it
    /// did. Returns the artifact after, with `changed`.
    ///
    /// # Errors
    /// `NotFound` for no live artifact `aid`; `unknown_tag` for a tag that
    /// names none, listing the tags; `too_many_tags` past
    /// [`MAX_ARTIFACT_TAGS`].
    pub fn edit_artifact_tags(
        &self,
        ctx: &AuditCtx,
        aid: &ArtifactId,
        edit: &TagEdit,
    ) -> Result<(Artifact, bool)> {
        self.with_tx(|tx| {
            let Some(before) = super::artifacts::live_artifact_in(tx, aid.as_str())? else {
                return Err(CoreError::NotFound);
            };
            let all = all_tags(tx)?;
            let mut ids: Vec<String> = before.tags.iter().map(|t| t.id.clone()).collect();
            match edit {
                TagEdit::Replace(refs) => {
                    ids = resolve_in(&all, refs)?.into_iter().map(|t| t.id).collect();
                }
                TagEdit::Change { add, remove } => {
                    let add = resolve_in(&all, add)?;
                    let remove = resolve_in(&all, remove)?;
                    for t in add {
                        if !ids.contains(&t.id) {
                            ids.push(t.id);
                        }
                    }
                    ids.retain(|i| !remove.iter().any(|t| t.id == *i));
                }
            }
            let mut want = ids.clone();
            want.sort();
            let mut had: Vec<String> = before.tags.iter().map(|t| t.id.clone()).collect();
            had.sort();
            if want == had {
                return Ok((before, false));
            }
            if want.len() > MAX_ARTIFACT_TAGS {
                return Err(CoreError::invalid(
                    "too_many_tags",
                    format!("an artifact carries at most {MAX_ARTIFACT_TAGS} tags"),
                ));
            }
            tx.execute(
                "DELETE FROM artifact_tags WHERE artifact_id = ?1",
                params![aid.as_str()],
            )?;
            let mut ins = tx.prepare_cached(
                "INSERT INTO artifact_tags (artifact_id, tag_id) VALUES (?1, ?2)",
            )?;
            for id in &want {
                ins.execute(params![aid.as_str(), id])?;
            }
            let after =
                super::artifacts::live_artifact_in(tx, aid.as_str())?.ok_or(CoreError::NotFound)?;
            let names: Vec<&str> = after.tags.iter().map(|t| t.name.as_str()).collect();
            let mut fields = serde_json::Map::new();
            fields.insert("tags".into(), serde_json::json!(names));
            let mut rec =
                AuditRecord::new(AuditKind::ArtifactUpdate, self.now()).with("fields", fields);
            rec.ids.artifact = Some(aid.as_str().to_string());
            self.record_audit(tx, ctx, rec)?;
            Ok((after, true))
        })
    }

    /// The IDs of the tags `refs` name, each by ID or by name as names are
    /// compared, for a filter: a ref that names no tag becomes a value no
    /// tag's ID or name can be (it starts with a control character), so the
    /// filter matches nothing (and says nothing about which tags exist).
    ///
    /// # Errors
    /// The store's.
    pub fn tag_filter_ids(&self, refs: &[String]) -> Result<Vec<String>> {
        if refs.is_empty() {
            return Ok(Vec::new());
        }
        let all = self.list_tags()?;
        Ok(refs
            .iter()
            .map(|r| match resolve_in(&all, std::slice::from_ref(r)) {
                Ok(mut t) => t.remove(0).id,
                Err(_) => format!("\u{1}{r}"),
            })
            .collect())
    }
}

/// Whether artifact tags `carried` match every one of `refs` (match all):
/// each ref is a tag's ID or its name as names are compared.
pub fn carries_all(carried: &[tags::TagRef], refs: &[String]) -> bool {
    refs.iter().all(|r| {
        let key = fold(r.trim());
        carried
            .iter()
            .any(|t| t.id == r.trim() || fold(&t.name) == key)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    const DAEMON: &AuditCtx = &AuditCtx::DAEMON;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-19T10:00:00Z"));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock).unwrap();
        (dir, st)
    }

    fn make(st: &Store, t: &str) -> ArtifactId {
        let req = serde_json::from_value(serde_json::json!({
            "title": t,
            "files": {"index.html": {"content": t, "encoding": "utf8"}}
        }))
        .unwrap();
        let p = clax_core::publish::validate(req).unwrap();
        ArtifactId::parse(&st.create_artifact(DAEMON, p, None).unwrap().0.id).unwrap()
    }

    fn tag(st: &Store, name: &str, colour: &str) -> Tag {
        st.create_tag(
            DAEMON,
            NewTag {
                name: name.into(),
                colour: Some(colour.into()),
                description: None,
            },
        )
        .unwrap()
    }

    fn code(e: CoreError) -> &'static str {
        match e {
            CoreError::Invalid { code, .. } => code,
            other => panic!("{other}"),
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn kinds(st: &Store) -> Vec<String> {
        st.events_after(0, 1000)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    #[test]
    fn the_migration_makes_the_tables_and_their_index() {
        let (_d, st) = store();
        let names: Vec<String> = st
            .with_read(|c| {
                let mut q = c.prepare(
                    "SELECT name FROM sqlite_master WHERE name IN
                       ('tags', 'artifact_tags', 'artifact_tags_by_tag') ORDER BY name",
                )?;
                let r = q.query_map([], |r| r.get(0))?;
                Ok(r.collect::<rusqlite::Result<Vec<String>>>()?)
            })
            .unwrap();
        assert_eq!(names, ["artifact_tags", "artifact_tags_by_tag", "tags"]);
    }

    #[test]
    fn names_are_unique_whatever_their_case_and_checked() {
        let (_d, st) = store();
        let d = tag(&st, "Design", "blue");
        assert_eq!(d.colour, "blue");
        let e = st
            .create_tag(
                DAEMON,
                NewTag {
                    name: "  design ".into(),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(code(e), "tag_exists");
        // Unicode folds too.
        tag(&st, "Ärger", "red");
        let e = st
            .create_tag(
                DAEMON,
                NewTag {
                    name: "ärger".into(),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(code(e), "tag_exists");
        for (name, colour, want) in [
            ("", None, "invalid_tag"),
            ("a\tb", None, "invalid_tag"),
            ("ok", Some("#fff"), "invalid_colour"),
        ] {
            let e = st
                .create_tag(
                    DAEMON,
                    NewTag {
                        name: name.into(),
                        colour: colour.map(str::to_string),
                        description: None,
                    },
                )
                .unwrap_err();
            assert_eq!(code(e), want, "{name:?}");
        }
        // The default colour.
        let plain = st
            .create_tag(
                DAEMON,
                NewTag {
                    name: "plain".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(plain.colour, DEFAULT_COLOUR);
        let names: Vec<String> = st
            .list_tags()
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, sorted(&["Ärger", "Design", "plain"]));
    }

    fn sorted(v: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = v.iter().map(|x| x.to_string()).collect();
        v.sort_by_key(|n| fold(n));
        v
    }

    #[test]
    fn a_rename_keeps_the_id_and_reaches_every_artifact() {
        let (_d, st) = store();
        let a = make(&st, "A");
        let t = tag(&st, "draft", "gray");
        tag(&st, "final", "green");
        st.edit_artifact_tags(DAEMON, &a, &TagEdit::Replace(s(&["draft"])))
            .unwrap();
        let after = st
            .update_tag(
                DAEMON,
                &t.id,
                TagPatch {
                    name: Some("Work in progress".into()),
                    colour: Some("orange".into()),
                    description: Some("not done".into()),
                },
            )
            .unwrap();
        assert_eq!(after.id, t.id);
        assert_eq!(st.tag_carriers(&t.id).unwrap(), vec![a.to_string()]);
        let got = st.get_artifact(&a).unwrap().unwrap().tags;
        assert_eq!(
            got,
            vec![tags::TagRef {
                id: t.id.clone(),
                name: "Work in progress".into(),
                colour: "orange".into()
            }]
        );
        // Its own name in another case is a rename, another's name a clash.
        st.update_tag(
            DAEMON,
            &t.id,
            TagPatch {
                name: Some("work in progress".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let e = st
            .update_tag(
                DAEMON,
                &t.id,
                TagPatch {
                    name: Some("FINAL".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(code(e), "tag_exists");
        // A blank description clears it; no change records nothing.
        let cleared = st
            .update_tag(
                DAEMON,
                &t.id,
                TagPatch {
                    description: Some(" ".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(cleared.description, None);
        let before = kinds(&st).len();
        st.update_tag(DAEMON, &t.id, TagPatch::default()).unwrap();
        assert_eq!(kinds(&st).len(), before);
        assert!(matches!(
            st.update_tag(DAEMON, "t_0000000000", TagPatch::default()),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn deleting_a_tag_removes_it_from_every_artifact() {
        let (_d, st) = store();
        let (a, b, c) = (make(&st, "A"), make(&st, "B"), make(&st, "C"));
        let t = tag(&st, "old", "red");
        tag(&st, "keep", "blue");
        for x in [&a, &b] {
            st.edit_artifact_tags(DAEMON, x, &TagEdit::Replace(s(&["old", "keep"])))
                .unwrap();
        }
        st.edit_artifact_tags(DAEMON, &c, &TagEdit::Replace(s(&["old"])))
            .unwrap();
        st.delete_artifact(DAEMON, &c).unwrap();
        let mut carried = st.delete_tag(DAEMON, &t.id).unwrap();
        carried.sort();
        let mut want = vec![a.to_string(), b.to_string()];
        want.sort();
        assert_eq!(carried, want, "a deleted artifact is not counted");
        for x in [&a, &b] {
            let names: Vec<String> = st
                .get_artifact(x)
                .unwrap()
                .unwrap()
                .tags
                .into_iter()
                .map(|t| t.name)
                .collect();
            assert_eq!(names, ["keep"]);
        }
        let left: usize = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM artifact_tags WHERE tag_id = ?1",
                    params![t.id],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(left, 0);
        assert!(st.get_tag(&t.id).unwrap().is_none());
        assert!(matches!(
            st.delete_tag(DAEMON, &t.id),
            Err(CoreError::NotFound)
        ));
        assert!(kinds(&st).contains(&"tag.delete".to_string()));
    }

    #[test]
    fn edits_add_remove_and_replace_and_record_only_a_change() {
        let (_d, st) = store();
        let a = make(&st, "A");
        let x = tag(&st, "x", "red");
        tag(&st, "Y", "blue");
        tag(&st, "z", "green");
        let names = |st: &Store| -> Vec<String> {
            st.get_artifact(&a)
                .unwrap()
                .unwrap()
                .tags
                .into_iter()
                .map(|t| t.name)
                .collect()
        };
        // By name in any case, or by ID; sorted by name.
        let (_, changed) = st
            .edit_artifact_tags(
                DAEMON,
                &a,
                &TagEdit::Replace(vec!["z".into(), x.id.clone()]),
            )
            .unwrap();
        assert!(changed);
        assert_eq!(names(&st), ["x", "z"]);
        let (art, changed) = st
            .edit_artifact_tags(
                DAEMON,
                &a,
                &TagEdit::Change {
                    add: s(&["y", "x"]),
                    remove: s(&["Z"]),
                },
            )
            .unwrap();
        assert!(changed);
        assert_eq!(
            art.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["x", "Y"]
        );
        let before = kinds(&st)
            .iter()
            .filter(|k| *k == "artifact.update")
            .count();
        assert_eq!(before, 2);
        let (_, changed) = st
            .edit_artifact_tags(
                DAEMON,
                &a,
                &TagEdit::Change {
                    add: s(&["x"]),
                    remove: s(&["z"]),
                },
            )
            .unwrap();
        assert!(!changed, "no change");
        assert_eq!(
            kinds(&st)
                .iter()
                .filter(|k| *k == "artifact.update")
                .count(),
            2
        );
        // Unknown tags are refused, naming the tags there are.
        let e = st
            .edit_artifact_tags(DAEMON, &a, &TagEdit::Replace(s(&["x", "nope"])))
            .unwrap_err();
        let msg = e.to_string();
        assert_eq!(code(e), "unknown_tag");
        assert!(msg.contains("'nope'") && msg.contains("x, Y, z"), "{msg}");
        assert_eq!(names(&st), ["x", "Y"], "nothing changed");
        // Clearing.
        st.edit_artifact_tags(DAEMON, &a, &TagEdit::Replace(vec![]))
            .unwrap();
        assert!(names(&st).is_empty());
        // No such artifact.
        let gone = make(&st, "gone");
        st.delete_artifact(DAEMON, &gone).unwrap();
        assert!(matches!(
            st.edit_artifact_tags(DAEMON, &gone, &TagEdit::Replace(s(&["x"]))),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn an_empty_list_says_the_owner_makes_tags() {
        let (_d, st) = store();
        let a = make(&st, "A");
        let e = st
            .edit_artifact_tags(DAEMON, &a, &TagEdit::Replace(s(&["x"])))
            .unwrap_err();
        assert!(e.to_string().contains("no tags yet"), "{e}");
    }

    #[test]
    fn an_artifact_carries_a_bounded_number_of_tags() {
        let (_d, st) = store();
        let a = make(&st, "A");
        let names: Vec<String> = (0..=MAX_ARTIFACT_TAGS).map(|i| format!("t{i}")).collect();
        for n in &names {
            tag(&st, n, "gray");
        }
        let e = st
            .edit_artifact_tags(DAEMON, &a, &TagEdit::Replace(names.clone()))
            .unwrap_err();
        assert_eq!(code(e), "too_many_tags");
        st.edit_artifact_tags(
            DAEMON,
            &a,
            &TagEdit::Replace(names[..MAX_ARTIFACT_TAGS].to_vec()),
        )
        .unwrap();
    }

    #[test]
    fn filters_match_all_and_unknown_tags_match_nothing() {
        let (_d, st) = store();
        let x = tag(&st, "x", "red");
        tag(&st, "y", "blue");
        let a = make(&st, "A");
        st.edit_artifact_tags(DAEMON, &a, &TagEdit::Replace(s(&["x", "y"])))
            .unwrap();
        let carried = st.get_artifact(&a).unwrap().unwrap().tags;
        assert!(carries_all(&carried, &s(&["X"])));
        assert!(carries_all(&carried, &[x.id.clone(), "y".into()]));
        assert!(!carries_all(&carried, &s(&["x", "z"])));
        assert!(carries_all(&carried, &[]));
        assert!(st.tag_filter_ids(&[]).unwrap().is_empty());
        let ids = st.tag_filter_ids(&s(&["X", "nope"])).unwrap();
        assert_eq!(ids[0], x.id);
        assert!(!tags::is_id(&ids[1]));
        assert!(!carries_all(&carried, &ids));
    }
}

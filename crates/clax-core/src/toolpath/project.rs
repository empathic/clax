//! Export projections (spec 2026-10-06-toolpath-audit-design §8): the
//! recorded history, selected, as a Toolpath `Graph` of artifact paths plus
//! the install path ([`project_artifacts`]), or as one linear audit-trail
//! path ([`project_journal`]), in JSON or JSONL.
//!
//! A projection reads its rows from a [`Source`] (the store's one read
//! transaction, [`Store::select`](crate::Store::select), or rows held in
//! memory) one path at a time, renders each with
//! [`render`](super::render) under the export's [`RenderEnv`], and writes
//! as it goes. Memory holds the `ActorDef` set of the path being written
//! and the list of paths, never a path's steps: a path object is written
//! as `{"steps": […], "path": {…, "head"}, "meta": {…, "actors"}}`, so its
//! head and actors follow its steps.
//!
//! Every refusal (a selector that names nothing, JSONL of more than one
//! path, an empty JSONL) happens before the first byte is written. The same
//! rows and the same request give the same bytes.
//!
//! **Selection.** Selectors of one kind union and kinds intersect. The
//! kinds are: the artifacts chosen (`--artifact` and `--live` together),
//! the sessions (`--by-session`), and the time range (`--since` inclusive,
//! `--until` exclusive, on the event's `at`). `--by-session` keeps the
//! steps of those sessions (a step whose `session_id` column names one, or
//! whose actor, or the agent a system actor acts for, is one), plus the
//! owner, viewer and anonymous steps (a person, or the person a system
//! actor acts for) on the artifacts those sessions touched; a system
//! actor's step that names no one in `for_actor` is kept only by its
//! session column. It filters steps and never changes the shape. Without an artifact chooser
//! the artifacts shape adds the install path: the steps on no artifact.
//!
//! **Refs.** A step on two artifacts (a moved thread, a copied snapshot, a
//! merged page) appears in both artifact paths under one step ID, each
//! copy with a `same-change` ref to the other (`toolpath:<path ID>/<step
//! ID>`) when the other is in the graph. A ref to an artifact whose path is
//! in the graph, other than the step's own, gains a `toolpath:<path ID>`
//! form beside its `clax://` one, and a tool call's `produced` ref names a
//! step in the graph by its `toolpath:` form instead. Every other ref keeps
//! its `clax://` form.

use super::{KIND_URI, Obj, Redaction, RenderEnv, clax_uri, merge_actor_def, render, step_id};
use crate::audit::{Actor, sha256_hex};
use crate::live::{PageKey, parse_page_url};
use crate::store::audit::AuditRow;
use crate::{CoreError, Result};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

/// What an export selects (spec §8.1), as the caller named it. No selector
/// means the whole install.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// Artifact IDs.
    pub artifacts: Vec<String>,
    /// Live pages, by page URL: the page whose origin and path the URL has.
    pub live_pages: Vec<String>,
    /// Clax session IDs or harness session IDs.
    pub by_sessions: Vec<String>,
    /// The earliest `at` kept: RFC 3339, or `YYYY-MM-DD` for 00:00 UTC.
    pub since: Option<String>,
    /// The `at` the range ends before, in the same forms.
    pub until: Option<String>,
}

/// An export's shape (spec §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    /// One path per artifact, plus the install path (spec §8.2).
    #[default]
    Artifacts,
    /// One linear audit-trail path of every selected step.
    Journal,
}

impl Shape {
    pub fn parse(s: &str) -> Option<Shape> {
        match s {
            "artifacts" => Some(Shape::Artifacts),
            "journal" => Some(Shape::Journal),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Shape::Artifacts => "artifacts",
            Shape::Journal => "journal",
        }
    }
}

/// An export's encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// One Toolpath `Graph` document.
    #[default]
    Json,
    /// One path as Toolpath JSONL (the JSONL RFC): only for a result of
    /// exactly one path.
    Jsonl,
}

impl Format {
    pub fn parse(s: &str) -> Option<Format> {
        match s {
            "json" => Some(Format::Json),
            "jsonl" => Some(Format::Jsonl),
            _ => None,
        }
    }
}

/// One export request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Export {
    pub selection: Selection,
    pub shape: Shape,
    pub format: Format,
    /// The redaction options; the default redacts nothing (spec §11).
    pub redaction: Redaction,
    /// Indent the JSON; refused for JSONL, whose records are one line each.
    pub pretty: bool,
}

/// What an export is rendered under: the rendering environment (the
/// install and the browser base of the daemon serving it) and the build
/// that exports, named in the graph's `meta.clax`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportEnv {
    pub render: RenderEnv,
    pub clax_version: String,
    pub clax_commit: String,
}

/// What a projection reads its rows through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    /// The rows whose `artifact_id` or `artifact2_id` is the artifact.
    Artifact(&'a str),
    /// The rows on no artifact.
    Install,
    /// Every row.
    All,
    /// Every row that may name one of these values in its session column,
    /// actor or body; a source may hand over more rows than that.
    Mentioning(&'a [String]),
}

/// What [`Source::artifact`] knows about an artifact now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactInfo {
    pub title: Option<String>,
    pub kind: Option<String>,
    /// A live page's origin and path.
    pub live: Option<PageKey>,
}

/// The rows a projection reads, and the little it needs to know about
/// artifacts. Every call sees the same snapshot.
pub trait Source {
    /// Calls `f` with each row in `scope`, in `seq` order, until it returns
    /// `false`.
    fn rows(&self, scope: Scope<'_>, f: &mut dyn FnMut(AuditRow) -> Result<bool>) -> Result<()>;
    /// The row numbered `seq`.
    fn row(&self, seq: i64) -> Result<Option<AuditRow>>;
    /// Every artifact ID a row names, in ID order.
    fn artifact_ids(&self) -> Result<Vec<String>>;
    /// Whether the artifact exists or any row names it.
    fn artifact_known(&self, id: &str) -> Result<bool>;
    /// The artifacts of the live page `key`: the page there now and any
    /// page merged away from there (each under its joined site's key
    /// origin too), and any page a `live.page` event records there.
    fn live_artifacts(&self, key: &PageKey) -> Result<Vec<String>>;
    /// What is known about artifact `id`.
    fn artifact(&self, id: &str) -> Result<ArtifactInfo>;
}

/// A selection resolved against a [`Source`].
#[derive(Debug, Clone)]
struct Resolved {
    /// The artifacts chosen, when an artifact or live selector is given.
    artifacts: Option<BTreeSet<String>>,
    /// The Clax sessions, when a session selector is given.
    sessions: Option<BTreeSet<String>>,
    /// The artifacts those sessions' steps touch.
    session_artifacts: BTreeSet<String>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    /// The selection as `graph.meta.clax.selection` names it.
    canonical: Value,
}

/// Where a row is being placed.
#[derive(Debug, Clone, Copy)]
enum Place<'a> {
    Artifact(&'a str),
    Install,
    Journal,
}

/// The time `s` names, for a selection bound: RFC 3339, or `YYYY-MM-DD`
/// for 00:00 UTC that day.
pub fn parse_bound(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).expect("midnight").and_utc());
    }
    Err(CoreError::invalid(
        "invalid_time",
        format!("'{s}' is not an RFC 3339 time or a YYYY-MM-DD date"),
    ))
}

fn show_time(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The actor that a step speaks for: its actor, or the person or agent a
/// system actor names in `for_actor`.
fn effective_actor(row: &AuditRow) -> Option<Actor> {
    let actor: Actor = serde_json::from_str(&row.actor).ok()?;
    if let Actor::System { .. } = actor
        && row.body.contains("\"for_actor\"")
        && let Ok(Value::Object(mut body)) = serde_json::from_str::<Value>(&row.body)
        && let Some(who) = body
            .remove("for_actor")
            .and_then(|v| serde_json::from_value::<Actor>(v).ok())
    {
        return Some(who);
    }
    Some(actor)
}

fn row_artifacts(row: &AuditRow) -> impl Iterator<Item = &str> {
    [&row.ids.artifact, &row.ids.artifact2]
        .into_iter()
        .flatten()
        .map(String::as_str)
}

impl Resolved {
    /// Whether `row` is selected where it is being placed.
    fn admits(&self, row: &AuditRow, place: Place<'_>) -> bool {
        if self.since.is_some() || self.until.is_some() {
            let Ok(at) = DateTime::parse_from_rfc3339(&row.at) else {
                return false;
            };
            let at = at.with_timezone(&Utc);
            if self.since.is_some_and(|s| at < s) || self.until.is_some_and(|u| at >= u) {
                return false;
            }
        }
        if let (Place::Journal, Some(chosen)) = (place, &self.artifacts)
            && !row_artifacts(row).any(|a| chosen.contains(a))
        {
            return false;
        }
        let Some(sessions) = &self.sessions else {
            return true;
        };
        if row
            .ids
            .session
            .as_ref()
            .is_some_and(|s| sessions.contains(s))
        {
            return true;
        }
        let who = effective_actor(row);
        if let Some(Actor::Agent(a)) = &who {
            return a.session_id.as_ref().is_some_and(|s| sessions.contains(s));
        }
        let human = matches!(
            who,
            Some(Actor::Owner { .. } | Actor::Viewer { .. } | Actor::Anonymous)
        );
        let on_theirs = match place {
            Place::Artifact(a) => self.session_artifacts.contains(a),
            Place::Install => false,
            Place::Journal => row_artifacts(row).any(|a| self.session_artifacts.contains(a)),
        };
        human && on_theirs
    }
}

/// Resolves `sel` against `src`, refusing a selector that names nothing.
fn resolve(src: &dyn Source, sel: &Selection) -> Result<Resolved> {
    let since = sel.since.as_deref().map(parse_bound).transpose()?;
    let until = sel.until.as_deref().map(parse_bound).transpose()?;
    if let (Some(s), Some(u)) = (since, until)
        && u <= s
    {
        return Err(CoreError::invalid(
            "invalid_range",
            format!(
                "until ({}) must be after since ({})",
                show_time(&u),
                show_time(&s)
            ),
        ));
    }

    let mut explicit = BTreeSet::new();
    for a in &sel.artifacts {
        let shaped = !a.is_empty()
            && a.len() <= 64
            && a.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !shaped {
            return Err(CoreError::invalid(
                "invalid_id",
                format!("'{a}' is not an artifact ID"),
            ));
        }
        if !src.artifact_known(a)? {
            return Err(CoreError::invalid(
                "unknown_artifact",
                format!("no artifact {a} is in this install's history"),
            ));
        }
        explicit.insert(a.clone());
    }
    let mut live = BTreeSet::new();
    let mut chosen = explicit.clone();
    for raw in &sel.live_pages {
        let key = parse_page_url(raw)?.key;
        let found = src.live_artifacts(&key)?;
        if found.is_empty() {
            return Err(CoreError::invalid(
                "unknown_live_page",
                format!(
                    "no live page at {} is in this install's history",
                    key.page_url()
                ),
            ));
        }
        chosen.extend(found);
        live.insert(key.page_url());
    }
    let artifacts = (!sel.artifacts.is_empty() || !sel.live_pages.is_empty()).then_some(chosen);

    let given: BTreeSet<String> = sel
        .by_sessions
        .iter()
        .map(|s| s.trim().to_string())
        .collect();
    let (sessions, session_artifacts) = if given.is_empty() {
        (None, BTreeSet::new())
    } else {
        sessions_of(src, &given)?
    };

    let canonical = json!({
        "artifacts": explicit,
        "live": live,
        "by_sessions": given,
        "since": since.as_ref().map(show_time),
        "until": until.as_ref().map(show_time),
    });
    Ok(Resolved {
        artifacts,
        sessions,
        session_artifacts,
        since,
        until,
        canonical,
    })
}

/// The Clax sessions `given` names (by Clax or harness session ID) in the
/// history, and the artifacts their steps touch; refuses a value that
/// names no session.
fn sessions_of(
    src: &dyn Source,
    given: &BTreeSet<String>,
) -> Result<(Option<BTreeSet<String>>, BTreeSet<String>)> {
    let values: Vec<String> = given.iter().cloned().collect();
    let mut matched: BTreeSet<String> = BTreeSet::new();
    let mut sessions: BTreeSet<String> = BTreeSet::new();
    src.rows(Scope::Mentioning(&values), &mut |row| {
        if let Some(s) = &row.ids.session
            && given.contains(s)
        {
            matched.insert(s.clone());
            sessions.insert(s.clone());
        }
        if let Some(Actor::Agent(a)) = effective_actor(&row)
            && let Some(sid) = &a.session_id
        {
            let by_harness = a.harness_session_id.as_ref().filter(|h| given.contains(*h));
            if let Some(h) = by_harness {
                matched.insert(h.clone());
            }
            if given.contains(sid) {
                matched.insert(sid.clone());
            }
            if given.contains(sid) || by_harness.is_some() {
                sessions.insert(sid.clone());
            }
        }
        Ok(true)
    })?;
    if let Some(missing) = given.iter().find(|g| !matched.contains(*g)) {
        return Err(CoreError::invalid(
            "unknown_session",
            format!(
                "no session {missing} (Clax or harness session ID) is in this install's history"
            ),
        ));
    }
    // The artifacts of every step of those sessions, now that a harness ID
    // is resolved to the Clax sessions it names.
    let ids: Vec<String> = sessions.iter().cloned().collect();
    let mut artifacts = BTreeSet::new();
    src.rows(Scope::Mentioning(&ids), &mut |row| {
        let theirs = row
            .ids
            .session
            .as_ref()
            .is_some_and(|s| sessions.contains(s))
            || matches!(effective_actor(&row), Some(Actor::Agent(a))
                if a.session_id.as_ref().is_some_and(|s| sessions.contains(s)));
        if theirs {
            artifacts.extend(row_artifacts(&row).map(str::to_string));
        }
        Ok(true)
    })?;
    Ok((Some(sessions), artifacts))
}

/// The first 12 hex characters of the SHA-256 of the canonical selection.
fn digest(canonical: &Value) -> String {
    sha256_hex(canonical.to_string().as_bytes())[..12].to_string()
}

fn install8(install: &str) -> &str {
    install.get(..8).unwrap_or(install)
}

fn artifact_path_id(a: &str) -> String {
    format!("clax-artifact-{a}")
}

/// The output, compact or indented, with the punctuation a projection
/// writes around the values serde writes.
struct Out<'w> {
    w: &'w mut dyn Write,
    pretty: bool,
}

impl Out<'_> {
    fn raw(&mut self, s: &str) -> Result<()> {
        self.w.write_all(s.as_bytes())?;
        Ok(())
    }

    /// A line break and `depth` levels of indentation, when pretty.
    fn nl(&mut self, depth: usize) -> Result<()> {
        if self.pretty {
            self.raw("\n")?;
            for _ in 0..depth {
                self.raw("  ")?;
            }
        }
        Ok(())
    }

    /// `"name":` (and a space, when pretty).
    fn key(&mut self, name: &str) -> Result<()> {
        self.raw(&serde_json::to_string(name).expect("a string serialises"))?;
        self.raw(if self.pretty { ": " } else { ":" })
    }

    /// `v`, indented as though it began at `depth`.
    fn value(&mut self, v: &Value, depth: usize) -> Result<()> {
        if self.pretty {
            let text = serde_json::to_string_pretty(v).expect("a value serialises");
            let pad = format!("\n{}", "  ".repeat(depth));
            self.raw(&text.replace('\n', &pad))
        } else {
            serde_json::to_writer(&mut *self.w, v).map_err(std::io::Error::from)?;
            Ok(())
        }
    }

    /// One JSONL record.
    fn line(&mut self, v: &Value) -> Result<()> {
        serde_json::to_writer(&mut *self.w, v).map_err(std::io::Error::from)?;
        self.raw("\n")
    }
}

/// The paths of the graph, decided before anything is written.
struct Plan {
    /// Artifact paths, in artifact ID order.
    artifacts: Vec<String>,
    install: bool,
    /// The journal path's ID, in the journal shape.
    journal_id: Option<String>,
    /// Whether the journal path holds a step.
    journal: bool,
}

impl Plan {
    fn has_artifact(&self, a: &str) -> bool {
        self.artifacts
            .binary_search_by(|x| x.as_str().cmp(a))
            .is_ok()
    }
}

/// Renders the rows of one path in turn, chaining them linearly and
/// collecting the path's actors (spec §7.2's merge).
struct PathState {
    prev: Option<i64>,
    first: Option<i64>,
    actors: BTreeMap<String, Value>,
}

impl PathState {
    fn new() -> PathState {
        PathState {
            prev: None,
            first: None,
            actors: BTreeMap::new(),
        }
    }

    /// Renders `row` as the next step, returning it and the actor
    /// definitions that are new or grew.
    fn step(
        &mut self,
        row: &AuditRow,
        env: &RenderEnv,
        opts: &Redaction,
    ) -> (Value, Vec<(String, Value)>) {
        let (step, actors) = match render(row, self.prev, env, opts) {
            Ok(r) => (r.step, r.actors),
            Err(e) => {
                tracing::error!(seq = row.seq, kind = %row.kind, error = %e, "an audit event could not be rendered for an export");
                (super::unrenderable_step(row, self.prev, env), Vec::new())
            }
        };
        let mut grew = Vec::new();
        for (actor, def) in actors {
            let merged = match self.actors.get(&actor) {
                Some(old) => merge_actor_def(old, &def),
                None => def,
            };
            if self.actors.get(&actor) != Some(&merged) {
                self.actors.insert(actor.clone(), merged.clone());
                grew.push((actor, merged));
            }
        }
        self.first.get_or_insert(row.seq);
        self.prev = Some(row.seq);
        (step, grew)
    }
}

/// Everything a projection works with.
struct Ctx<'a> {
    src: &'a dyn Source,
    sel: Resolved,
    req: &'a Export,
    env: &'a ExportEnv,
    plan: Plan,
    graph_id: String,
}

impl Ctx<'_> {
    fn install_uri(&self) -> String {
        clax_uri(&self.env.render.install, Obj::Install)
    }

    fn install_path_id(&self) -> String {
        format!("clax-install-{}", install8(&self.env.render.install))
    }

    /// The path ID `row` is placed in within this graph, when it is in it:
    /// the first of its artifacts' paths, the install path, or the
    /// journal path.
    fn path_of(&self, row: &AuditRow) -> Option<String> {
        if let Some(j) = &self.plan.journal_id {
            return self.sel.admits(row, Place::Journal).then(|| j.clone());
        }
        let mut arts: Vec<&str> = row_artifacts(row).collect();
        arts.sort_unstable();
        if arts.is_empty() {
            return (self.plan.install && self.sel.admits(row, Place::Install))
                .then(|| self.install_path_id());
        }
        arts.into_iter()
            .find(|a| self.plan.has_artifact(a) && self.sel.admits(row, Place::Artifact(a)))
            .map(artifact_path_id)
    }

    /// Adds the graph-relative forms of `step`'s refs, placed in `place`:
    /// `same-change` to its copy in another artifact path, `toolpath:`
    /// forms of refs to other artifact paths, and `produced` steps by
    /// their `toolpath:` form.
    fn link(&self, row: &AuditRow, step: &mut Value, place: Place<'_>) -> Result<()> {
        let install = &self.env.render.install;
        let step_prefix = format!("{}/step/e", self.install_uri());
        let artifact_prefix = format!("{}/a/", self.install_uri());
        let own = match place {
            Place::Artifact(a) => Some(a),
            _ => None,
        };
        let Some(meta) = step.get_mut("meta").and_then(Value::as_object_mut) else {
            return Ok(());
        };
        let mut refs: Vec<Value> = match meta.remove("refs") {
            Some(Value::Array(r)) => r,
            _ => Vec::new(),
        };
        let mut out = Vec::with_capacity(refs.len() + 2);
        for mut r in refs.drain(..) {
            let rel = r["rel"].as_str().unwrap_or_default().to_string();
            let href = r["href"].as_str().unwrap_or_default().to_string();
            if rel == "produced"
                && let Some(seq) = href
                    .strip_prefix(&step_prefix)
                    .and_then(|n| n.parse::<i64>().ok())
                && let Some(produced) = self.src.row(seq)?
                && let Some(path) = self.path_of(&produced)
            {
                r["href"] = format!("toolpath:{path}/{}", step_id(seq)).into();
                out.push(r);
                continue;
            }
            let other = href
                .strip_prefix(&artifact_prefix)
                .filter(|x| self.plan.has_artifact(x) && Some(*x) != own)
                .filter(|x| clax_uri(install, Obj::Artifact(x)) == href)
                .map(str::to_string);
            out.push(r);
            if self.plan.journal_id.is_none()
                && let Some(x) = other
            {
                out.push(json!({"rel": rel, "href": format!("toolpath:{}", artifact_path_id(&x))}));
            }
        }
        if let Some(a) = own {
            for other in row_artifacts(row).filter(|x| *x != a) {
                if self.plan.has_artifact(other) && self.sel.admits(row, Place::Artifact(other)) {
                    out.push(json!({
                        "rel": "same-change",
                        "href": format!("toolpath:{}/{}", artifact_path_id(other), step_id(row.seq)),
                    }));
                }
            }
        }
        if !out.is_empty() {
            meta.insert("refs".into(), out.into());
        }
        Ok(())
    }

    fn scope_of<'p>(place: Place<'p>) -> Scope<'p> {
        match place {
            Place::Artifact(a) => Scope::Artifact(a),
            Place::Install => Scope::Install,
            Place::Journal => Scope::All,
        }
    }

    /// Whether the path at `place` would hold a step.
    fn nonempty(&self, place: Place<'_>) -> Result<bool> {
        let mut any = false;
        self.src.rows(Self::scope_of(place), &mut |row| {
            any = self.sel.admits(&row, place);
            Ok(!any)
        })?;
        Ok(any)
    }

    /// Calls `f` with each selected row of the path at `place`, in `seq`
    /// order.
    fn each(&self, place: Place<'_>, f: &mut dyn FnMut(AuditRow) -> Result<()>) -> Result<()> {
        self.src.rows(Self::scope_of(place), &mut |row| {
            if self.sel.admits(&row, place) {
                f(row)?;
            }
            Ok(true)
        })
    }

    /// The `meta` of the path at `place`, without its actors.
    fn path_meta(&self, place: Place<'_>) -> Result<Value> {
        let source = self.base_uri(place);
        Ok(match place {
            Place::Artifact(a) => {
                let info = self.src.artifact(a)?;
                let title = info
                    .title
                    .filter(|t| !t.is_empty() && !self.req.redaction.no_text)
                    .unwrap_or_else(|| format!("Artifact {a}"));
                let mut clax = json!({"projection": "artifact", "artifact_id": a});
                if let Some(k) = info.kind {
                    clax["artifact_kind"] = k.into();
                }
                if let Some(key) = info.live {
                    clax["origin"] = key.origin.into();
                    clax["path"] = key.path.into();
                }
                let mut meta = json!({
                    "title": title, "kind": KIND_URI, "source": source, "clax": clax,
                });
                if let Some(base) = &self.env.render.view_base {
                    meta["refs"] = json!([{
                        "rel": "view",
                        "href": format!("{}/a/{}", base.trim_end_matches('/'), super::seg(a)),
                    }]);
                }
                meta
            }
            Place::Install => json!({
                "title": "Clax install audit trail", "kind": KIND_URI, "source": source,
                "clax": {"projection": "install"},
            }),
            Place::Journal => json!({
                "title": "Clax audit trail export", "kind": KIND_URI, "source": source,
                "clax": {"projection": "journal"},
            }),
        })
    }

    fn base_uri(&self, place: Place<'_>) -> String {
        match place {
            Place::Artifact(a) => clax_uri(&self.env.render.install, Obj::Artifact(a)),
            Place::Install | Place::Journal => self.install_uri(),
        }
    }

    fn path_id(&self, place: Place<'_>) -> String {
        match place {
            Place::Artifact(a) => artifact_path_id(a),
            Place::Install => self.install_path_id(),
            Place::Journal => self.plan.journal_id.clone().expect("a journal plan"),
        }
    }

    /// What the export was: its selection, shape, redaction and build.
    fn export_clax(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("install".into(), self.env.render.install.as_str().into());
        m.insert("clax_version".into(), self.env.clax_version.as_str().into());
        m.insert("clax_commit".into(), self.env.clax_commit.as_str().into());
        m.insert("selection".into(), self.sel.canonical.clone());
        m.insert("shape".into(), self.req.shape.name().into());
        m.insert("redaction".into(), json!(self.req.redaction.names()));
        m
    }

    /// Writes the path at `place` as one element of `paths`, at `depth`.
    fn write_path(
        &self,
        out: &mut Out<'_>,
        place: Place<'_>,
        depth: usize,
        seqs: &mut (Option<i64>, Option<i64>),
    ) -> Result<()> {
        let env = &self.env.render;
        let opts = &self.req.redaction;
        let mut st = PathState::new();
        out.raw("{")?;
        out.nl(depth + 1)?;
        out.key("steps")?;
        out.raw("[")?;
        let mut n = 0usize;
        self.each(place, &mut |row| {
            let (mut step, _) = st.step(&row, env, opts);
            self.link(&row, &mut step, place)?;
            if n > 0 {
                out.raw(",")?;
            }
            out.nl(depth + 2)?;
            out.value(&step, depth + 2)?;
            n += 1;
            Ok(())
        })?;
        if n > 0 {
            out.nl(depth + 1)?;
        }
        out.raw("],")?;
        let head = st.prev.expect("a planned path holds a step");
        seqs.0 = Some(seqs.0.map_or(st.first.unwrap_or(head), |f| {
            f.min(st.first.unwrap_or(head))
        }));
        seqs.1 = Some(seqs.1.map_or(head, |l| l.max(head)));
        out.nl(depth + 1)?;
        out.key("path")?;
        out.value(
            &json!({"id": self.path_id(place), "base": {"uri": self.base_uri(place)}, "head": step_id(head)}),
            depth + 1,
        )?;
        out.raw(",")?;
        out.nl(depth + 1)?;
        out.key("meta")?;
        let mut meta = self.path_meta(place)?;
        meta["actors"] = json!(st.actors);
        out.value(&meta, depth + 1)?;
        out.nl(depth)?;
        out.raw("}")
    }

    /// Writes the graph of `places`.
    fn write_graph(&self, out: &mut Out<'_>, places: &[Place<'_>]) -> Result<()> {
        let mut seqs = (None, None);
        out.raw("{")?;
        out.nl(1)?;
        out.key("graph")?;
        out.value(&json!({"id": self.graph_id}), 1)?;
        out.raw(",")?;
        out.nl(1)?;
        out.key("paths")?;
        out.raw("[")?;
        for (i, place) in places.iter().enumerate() {
            if i > 0 {
                out.raw(",")?;
            }
            out.nl(2)?;
            self.write_path(out, *place, 2, &mut seqs)?;
        }
        if !places.is_empty() {
            out.nl(1)?;
        }
        out.raw("],")?;
        out.nl(1)?;
        out.key("meta")?;
        let mut clax = self.export_clax();
        clax.insert("first_seq".into(), json!(seqs.0));
        clax.insert("last_seq".into(), json!(seqs.1));
        out.value(
            &json!({
                "title": "Clax export",
                "refs": [{"rel": "source", "href": self.install_uri()}],
                "clax": clax,
            }),
            1,
        )?;
        out.nl(0)?;
        out.raw("}")?;
        if out.pretty {
            out.raw("\n")?;
        }
        Ok(())
    }

    /// Writes the path at `place` as Toolpath JSONL: `PathOpen`, each
    /// actor's `ActorDef` before its first step and again whenever its
    /// definition grows, the steps, `Head` and `PathClose`.
    fn write_jsonl(&self, out: &mut Out<'_>, place: Place<'_>) -> Result<()> {
        let env = &self.env.render;
        let opts = &self.req.redaction;
        let mut meta = self.path_meta(place)?;
        if let Some(clax) = meta.get_mut("clax").and_then(Value::as_object_mut) {
            clax.extend(self.export_clax());
        }
        out.line(&json!({"PathOpen": {
            "version": "1",
            "id": self.path_id(place),
            "base": {"uri": self.base_uri(place)},
            "meta": meta,
        }}))?;
        let mut st = PathState::new();
        self.each(place, &mut |row| {
            let (mut step, grew) = st.step(&row, env, opts);
            self.link(&row, &mut step, place)?;
            for (actor, definition) in grew {
                out.line(&json!({"ActorDef": {"actor": actor, "definition": definition}}))?;
            }
            out.line(&json!({"Step": step}))
        })?;
        let head = st.prev.expect("a planned path holds a step");
        out.line(&json!({"Head": {"step_id": step_id(head)}}))?;
        out.line(&json!({"PathClose": {}}))
    }
}

/// Plans and checks an export before anything is written.
fn plan<'a>(src: &'a dyn Source, req: &'a Export, env: &'a ExportEnv) -> Result<Ctx<'a>> {
    if req.pretty && req.format == Format::Jsonl {
        return Err(CoreError::invalid(
            "invalid_option",
            "pretty applies to JSON only; a JSONL record is one line",
        ));
    }
    let sel = resolve(src, &req.selection)?;
    let digest = digest(&sel.canonical);
    let graph_id = format!("clax-{}-{digest}", install8(&env.render.install));
    if req.format == Format::Jsonl
        && req.shape == Shape::Artifacts
        && sel.artifacts.as_ref().is_none_or(|a| a.len() != 1)
    {
        return Err(CoreError::invalid(
            "jsonl_needs_one_path",
            "JSONL holds exactly one path: export with shape journal (--shape journal), or select exactly one artifact",
        ));
    }
    let mut ctx = Ctx {
        src,
        sel,
        req,
        env,
        plan: Plan {
            artifacts: Vec::new(),
            install: false,
            journal_id: None,
            journal: false,
        },
        graph_id,
    };
    match req.shape {
        Shape::Journal => {
            ctx.plan.journal_id = Some(format!("clax-export-{digest}"));
            ctx.plan.journal = ctx.nonempty(Place::Journal)?;
        }
        Shape::Artifacts => {
            let candidates: Vec<String> = match (&ctx.sel.artifacts, &ctx.sel.sessions) {
                (Some(chosen), _) => chosen.iter().cloned().collect(),
                (None, Some(_)) => ctx.sel.session_artifacts.iter().cloned().collect(),
                (None, None) => src.artifact_ids()?,
            };
            let mut planned = Vec::new();
            for a in candidates {
                if ctx.nonempty(Place::Artifact(&a))? {
                    planned.push(a);
                }
            }
            ctx.plan.artifacts = planned;
            ctx.plan.install = ctx.sel.artifacts.is_none() && ctx.nonempty(Place::Install)?;
        }
    }
    if req.format == Format::Jsonl {
        let empty = match req.shape {
            Shape::Journal => !ctx.plan.journal,
            Shape::Artifacts => ctx.plan.artifacts.is_empty(),
        };
        if empty {
            return Err(CoreError::invalid(
                "empty_selection",
                "the selection holds no recorded event, and a JSONL path needs at least one step",
            ));
        }
    }
    Ok(ctx)
}

/// Writes the artifacts shape of `req` (spec §8.2): a `Graph` of one path
/// per selected artifact that holds a selected step, in artifact ID order,
/// then, without an artifact selector, the install path; or, as JSONL, the
/// one artifact path.
pub fn project_artifacts(
    src: &dyn Source,
    req: &Export,
    env: &ExportEnv,
    w: &mut dyn Write,
) -> Result<()> {
    let req = Export {
        shape: Shape::Artifacts,
        ..req.clone()
    };
    let ctx = plan(src, &req, env)?;
    write(&ctx, w)
}

/// Writes the journal shape of `req`: one linear audit-trail path of every
/// selected step, shaped like a journal segment, with ID
/// `clax-export-<digest>`, as a `Graph` or as JSONL.
pub fn project_journal(
    src: &dyn Source,
    req: &Export,
    env: &ExportEnv,
    w: &mut dyn Write,
) -> Result<()> {
    let req = Export {
        shape: Shape::Journal,
        ..req.clone()
    };
    let ctx = plan(src, &req, env)?;
    write(&ctx, w)
}

/// Writes `req` in its shape: [`project_artifacts`] or [`project_journal`].
pub fn export(src: &dyn Source, req: &Export, env: &ExportEnv, w: &mut dyn Write) -> Result<()> {
    let ctx = plan(src, req, env)?;
    write(&ctx, w)
}

fn write(ctx: &Ctx<'_>, w: &mut dyn Write) -> Result<()> {
    let mut out = Out {
        w,
        pretty: ctx.req.pretty,
    };
    let places: Vec<Place<'_>> = match &ctx.plan.journal_id {
        Some(_) => ctx
            .plan
            .journal
            .then_some(Place::Journal)
            .into_iter()
            .collect(),
        None => ctx
            .plan
            .artifacts
            .iter()
            .map(|a| Place::Artifact(a))
            .chain(ctx.plan.install.then_some(Place::Install))
            .collect(),
    };
    match ctx.req.format {
        Format::Json => ctx.write_graph(&mut out, &places)?,
        Format::Jsonl => ctx.write_jsonl(&mut out, places[0])?,
    }
    out.w.flush()?;
    Ok(())
}

/// Rows held in memory, as a [`Source`]: for tests and goldens.
#[derive(Debug, Clone, Default)]
pub struct MemSource {
    pub rows: Vec<AuditRow>,
    pub info: BTreeMap<String, ArtifactInfo>,
}

impl Source for MemSource {
    fn rows(&self, scope: Scope<'_>, f: &mut dyn FnMut(AuditRow) -> Result<bool>) -> Result<()> {
        for r in &self.rows {
            let take = match scope {
                Scope::Artifact(a) => row_artifacts(r).any(|x| x == a),
                Scope::Install => row_artifacts(r).next().is_none(),
                Scope::All | Scope::Mentioning(_) => true,
            };
            if take && !f(r.clone())? {
                break;
            }
        }
        Ok(())
    }

    fn row(&self, seq: i64) -> Result<Option<AuditRow>> {
        Ok(self.rows.iter().find(|r| r.seq == seq).cloned())
    }

    fn artifact_ids(&self) -> Result<Vec<String>> {
        let ids: BTreeSet<String> = self
            .rows
            .iter()
            .flat_map(|r| row_artifacts(r).map(str::to_string))
            .collect();
        Ok(ids.into_iter().collect())
    }

    fn artifact_known(&self, id: &str) -> Result<bool> {
        Ok(self.info.contains_key(id)
            || self.rows.iter().any(|r| row_artifacts(r).any(|a| a == id)))
    }

    fn live_artifacts(&self, key: &PageKey) -> Result<Vec<String>> {
        let mut found: BTreeSet<String> = self
            .info
            .iter()
            .filter(|(_, i)| i.live.as_ref() == Some(key))
            .map(|(a, _)| a.clone())
            .collect();
        for r in self.rows.iter().filter(|r| r.kind == "live.page") {
            let body: Value = serde_json::from_str(&r.body).unwrap_or_default();
            if r.ids.origin.as_deref() == Some(key.origin.as_str())
                && body["path"].as_str() == Some(key.path.as_str())
                && let Some(a) = &r.ids.artifact
            {
                found.insert(a.clone());
            }
        }
        Ok(found.into_iter().collect())
    }

    fn artifact(&self, id: &str) -> Result<ArtifactInfo> {
        Ok(self.info.get(id).cloned().unwrap_or_default())
    }
}

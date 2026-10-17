//! The JSON view of a thread, as each reader reads it, and what an agent
//! channel may see of it ([`Vis`]).

use crate::state::AppState;
use clax_core::ArtifactId;
use clax_core::model::{Artifact, Asset, Comment, Thread, Version};
use clax_core::visibility::UNCONNECTED;
use clax_core::visibility::placeholder::{self, ApprovalState, WithheldAuthor};
use clax_core::visibility::{self, Author, Reader};
use clax_core::working::{SessionWorking, WorkingView};
use clax_store::docs::{Acquired, Doc, DocPage, Written};
use clax_store::live::LivePage;
use clax_store::threads::ThreadOwner;
use clax_store::visibility::Visibility;
use clax_store::{DeliveryViewer, Store};
use serde_json::{Value, json};
use std::collections::HashMap;

/// The thread as the owner reads it (the live page routes, which only the
/// owner reaches): the stored fields plus `clip_url`,
/// `clip_path` (only when `with_path`, that is the caller presented the
/// token, and the clip is a file on this machine), `feedback_state`, and `resolved_by_name`: the current display name
/// of the viewer named by `resolved_by` (not the name at the resolve), or
/// `null` when that viewer has none or the thread was not resolved by a
/// viewer; and `addressed_pending`, `{harness, at}` while an agent's address
/// of a live page's thread waits for the page's next snapshot, else `null`.
/// A live page's thread also carries `page_path` (the path it was made at:
/// its page's, unless a merge rule mapped it there), `page_url` (the origin,
/// that path and the thread's route), `moves` (its moves between pages,
/// oldest first) and `snapshot_path` (its version's `index.html` on disk,
/// only when `with_path` and the version is kept in files on this machine,
/// else `null`).
pub fn thread_view(
    st: &Store,
    t: &Thread,
    codex_push: bool,
    with_path: bool,
) -> clax_core::Result<Value> {
    Ok(
        thread_views(st, std::slice::from_ref(t), codex_push, with_path)?
            .pop()
            .expect("one view per thread"),
    )
}

/// [`thread_view`] of each of `threads`, in order, read in one store call.
pub fn thread_views(
    st: &Store,
    threads: &[Thread],
    codex_push: bool,
    with_path: bool,
) -> clax_core::Result<Vec<Value>> {
    thread_views_as(st, threads, codex_push, with_path, &DeliveryViewer::Owner)
}

/// The views a `thread` event carries, which reach every subscriber of the
/// artifact: no `clip_path`, and no `feedback_state` (each reader hears
/// its own in `feedback_state` events, spec 2026-10-08 §10), so a client
/// keeps the one it holds.
pub fn thread_event_views(
    st: &Store,
    threads: &[Thread],
    codex_push: bool,
) -> clax_core::Result<Vec<Value>> {
    let mut views = thread_views_as(st, threads, codex_push, false, &DeliveryViewer::Visitor)?;
    for v in &mut views {
        if let Some(o) = v.as_object_mut() {
            o.remove("feedback_state");
        }
    }
    Ok(views)
}

/// [`thread_views`] as `viewer` reads them: each `feedback_state` its
/// [`clax_store::Delivery::view`].
pub fn thread_views_as(
    st: &Store,
    threads: &[Thread],
    codex_push: bool,
    with_path: bool,
    viewer: &DeliveryViewer,
) -> clax_core::Result<Vec<Value>> {
    Ok(views_and_senders(st, threads, codex_push, with_path, viewer)?.0)
}

/// Each thread view's batch senders, in `sends` order
/// ([`clax_store::batches::ThreadSend::sender`], never served).
type Senders = Vec<Vec<Option<String>>>;

/// Each thread view's owner agent, when it has one.
type Owners = Vec<Option<ThreadOwner>>;

/// [`thread_views_as`], with each view's batch senders and owner.
fn views_and_senders(
    st: &Store,
    threads: &[Thread],
    codex_push: bool,
    with_path: bool,
    viewer: &DeliveryViewer,
) -> clax_core::Result<(Vec<Value>, Senders, Owners)> {
    let extras = st.thread_extras(threads, codex_push)?;
    let senders: Senders = extras
        .iter()
        .map(|x| x.sends.iter().map(|b| b.sender.clone()).collect())
        .collect();
    let owners: Owners = extras.iter().map(|x| x.owner.clone()).collect();
    // Each owner's person's name, once per person (§11 "Display").
    let mut person_names: HashMap<Option<String>, String> = HashMap::new();
    for o in owners.iter().flatten() {
        if !person_names.contains_key(&o.person) {
            let p = st.agent_person(o.person.as_deref())?;
            person_names.insert(o.person.clone(), p.name);
        }
    }
    let mut live: HashMap<&str, Option<LivePage>> = HashMap::new();
    for t in threads {
        if !live.contains_key(t.artifact_id.as_str()) {
            let p = st.live_page_of(&ArtifactId::parse(&t.artifact_id)?)?;
            live.insert(&t.artifact_id, p);
        }
    }
    threads
        .iter()
        .zip(extras)
        .map(|(t, x)| {
            let mut v = serde_json::to_value(t).expect("threads serialise");
            v["clip_url"] = if t.has_clip {
                json!(format!(
                    "/api/artifacts/{}/threads/{}/clip",
                    t.artifact_id, t.id
                ))
            } else {
                Value::Null
            };
            let id = ArtifactId::parse(&t.artifact_id)?;
            v["clip_path"] = match (t.has_clip && with_path)
                .then(|| st.local_clip(&id, &t.id))
                .flatten()
            {
                Some(path) => json!(path.to_string_lossy()),
                None => Value::Null,
            };
            v["feedback_state"] = json!(x.delivery.as_ref().and_then(|d| d.view(viewer)));
            v["addressed_in"] = json!(x.addressed_in);
            v["sends"] = json!(x.sends);
            v["resolved_by_name"] = json!(x.resolved_by_name);
            v["addressed_pending"] = match x.addressed_pending {
                Some((harness, at)) => json!({"harness": harness, "at": at}),
                None => Value::Null,
            };
            // Its twin on a remote: the thread's, then each comment's.
            if let Some(l) = &x.linked {
                for (i, c) in t.comments.iter().enumerate() {
                    if let Some(lc) = l.comments.get(&c.id) {
                        v["comments"][i]["linked"] = json!(lc);
                    }
                }
                let mut lv = json!(l);
                if let Some(m) = lv.as_object_mut() {
                    m.remove("comments");
                }
                v["linked"] = lv;
            }
            // The owner's handle only through [`Vis`], to those it may name.
            if let Some(o) = &x.owner {
                v["owner"] = json!({"name": o.name, "person": o.person, "live": o.live,
                    "person_name": person_names.get(&o.person)});
            }
            if let Some(Some(p)) = live.get(t.artifact_id.as_str()) {
                let path = x.live_path.as_deref().unwrap_or(&p.path);
                v["page_path"] = json!(path);
                v["page_url"] = json!(format!(
                    "{}{}{}",
                    p.origin,
                    path,
                    t.anchor.route.as_deref().unwrap_or("")
                ));
                v["moves"] = json!(x.moves);
                v["snapshot_path"] = match with_path
                    .then(|| {
                        st.content()
                            .local_file(&id, t.version_n, clax_core::publish::INDEX)
                    })
                    .flatten()
                {
                    Some(path) => json!(path.to_string_lossy()),
                    None => Value::Null,
                };
            }
            Ok(v)
        })
        .collect::<clax_core::Result<Vec<Value>>>()
        .map(|views| (views, senders, owners))
}

/// Comment `c`'s view `view` as an agent reads it: a viewer's name says
/// `(not connected)` when the viewer is not (spec §9.6, §19 item 7); an
/// agent's reply from a named session reads `<name> (agent, via
/// <harness>)` when the agent is the reader's own person's, else `<name>
/// (<person>'s agent, via <harness>)` (spec §11 "Display", §19 item 1),
/// with no `agent` object; one from a session with no name keeps its
/// stored name.
fn mark_comment(vis: &Visibility, c: &Comment, view: &mut Value) {
    if view["linked"]["origin"] == "remote" {
        // A remote's author says where they are, never a local person.
        if let (Some(name), Some(remote)) = (
            view["linked"]["author_name"].as_str(),
            view["linked"]["remote"].as_str(),
        ) {
            view["author_name"] = json!(clax_store::linked::remote_label(name, remote));
        }
        return;
    }
    if c.author_kind == clax_store::threads::AUTHOR_AGENT {
        if let (Some(a), Some(harness)) = (&c.agent, &c.via_harness) {
            let reader = match vis.reader() {
                Some(Reader::Person(p)) => Some(p.as_str()),
                _ => None,
            };
            let person = (c.agent_person.as_deref() != reader).then_some(a.person_name.as_str());
            view["author_name"] =
                json!(clax_core::feedback::agent_author(&a.name, person, harness));
        }
        if let Some(o) = view.as_object_mut() {
            o.remove("agent");
        }
        return;
    }
    let author = Author::of_viewer(c.author_public_id.as_deref());
    if let Some(name) = view["author_name"].as_str() {
        view["author_name"] = json!(vis.label(name, &author));
    }
}

/// Thread `t`'s view `view` as an agent reads it: every name of an author
/// who is not connected says so (spec §9.6): its viewers' comments, its
/// batches' senders (`senders`, in `sends` order) and the viewer who
/// resolved it; its agents' replies name their agents ([`mark_comment`]).
fn mark_thread(vis: &Visibility, t: &Thread, view: &mut Value, senders: &[Option<String>]) {
    for (i, c) in t.comments.iter().enumerate() {
        mark_comment(vis, c, &mut view["comments"][i]);
    }
    for (i, sender) in senders.iter().enumerate() {
        let send = &mut view["sends"][i];
        if let Some(name) = send["sent_by"].as_str() {
            let mut name = name.to_string();
            for a in visibility::authors_of_batch_note(sender.as_deref()) {
                name = vis.label(&name, &a);
            }
            send["sent_by"] = json!(name);
        }
    }
    if let (Some(by), Some(name)) = (
        t.resolved_by
            .as_deref()
            .filter(|b| b.starts_with("viewer:")),
        view["resolved_by_name"].as_str(),
    ) {
        let name = match visibility::authors_of_resolved_by(by, None).first() {
            Some(a) => vis.label(name, a),
            None => name.to_string(),
        };
        view["resolved_by_name"] = json!(name);
    }
}

/// Viewer views `people` (each with its public ID at `id_key` and its
/// `display_name`) as an agent reads them: each named viewer who is not
/// connected says so (spec §9.6).
fn mark_viewers(vis: &Visibility, people: &mut Value, id_key: &str) {
    let Some(people) = people.as_array_mut() else {
        return;
    };
    for p in people {
        if let (Some(id), Some(name)) = (p[id_key].as_str(), p["display_name"].as_str()) {
            let name = vis.label(name, &Author::of_viewer(Some(id)));
            p["display_name"] = json!(name);
        }
    }
}

/// An artifact view's `participants` as an agent reads them.
fn mark_participants(vis: &Visibility, view: &mut Value) {
    mark_viewers(vis, &mut view["participants"]["people"], "public_id");
    // Agents' people by their payload-safe names.
    if let Some(agents) = view["participants"]["agents"].as_array_mut() {
        for a in agents {
            if let Some(n) = a["person"]["name"].as_str().map(str::to_string) {
                let n = clax_core::feedback::display_name(&n);
                if a.get("via").is_some_and(|v| v.is_string()) {
                    a["via"] = json!(clax_store::attention::relay_via(&n));
                }
                a["person"]["name"] = json!(n);
            }
        }
    }
}

/// The visibility an agent channel's response goes through (spec §8):
/// the caller's reader and the visibility generation read when the
/// request began, before the store reads that build the view (M17).
#[derive(Clone, Debug)]
pub struct VisFor {
    pub reader: Option<Reader>,
    pub generation: u64,
    /// For an agent channel, its session (`None` when it names none):
    /// thread views say `taken_by` when another live agent owns one.
    pub agent: Option<Option<String>>,
    /// Whose agents' handles thread views name in `owner`.
    pub handles: Handles,
    /// How thread views read their delivery state ([`clax_store::Delivery::view`]).
    pub viewer: DeliveryViewer,
    /// For an agent on a hosted daemon: how its thread views name a shown
    /// clip's keyed address (`clip_link`).
    pub clip_links: Option<ClipLinks>,
}

/// How a hosted daemon names a thread's clip at its keyed address
/// ([`crate::hosted::clip_link`]) in the thread views an agent reads.
#[derive(Clone)]
pub struct ClipLinks {
    hosted: std::sync::Arc<crate::hosted::Hosted>,
    token: String,
    live: std::sync::Arc<crate::live::LiveIds>,
}

impl ClipLinks {
    /// On a hosted daemon, its clip links; `None` on a local one.
    pub fn of(s: &AppState) -> Option<ClipLinks> {
        s.hosted.as_ref().map(|h| ClipLinks {
            hosted: h.clone(),
            token: s.token.clone(),
            live: s.live_ids.clone(),
        })
    }

    /// The keyed address of thread `tid`'s clip on artifact `aid`; `None`
    /// for a live page's, which the address does not serve to a caller
    /// with no credential (`crate::routes::threads::keyed_clip`).
    pub fn link(&self, aid: &str, tid: &str) -> Option<String> {
        (!self.live.contains(aid))
            .then(|| crate::hosted::clip_link_at(&self.hosted, &self.token, aid, tid))
    }
}

impl std::fmt::Debug for ClipLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipLinks")
            .field("origin", &self.hosted.origin)
            .finish_non_exhaustive()
    }
}

/// How `caller` reads threads' delivery states (spec 2026-10-08 §10,
/// §18): the owner by hand or in a browser as the owner, a connected
/// person's browser as that person, an agent as its session (every row
/// is decided at [`VisFor::build`]), anyone else as a visitor.
pub fn delivery_viewer(caller: &crate::auth::Caller) -> DeliveryViewer {
    use crate::auth::Caller;
    match caller {
        Caller::Owner { .. } => DeliveryViewer::Owner,
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => DeliveryViewer::Agent {
            session: caller.session().map(str::to_string),
            every: false,
        },
        Caller::Person { viewer } => DeliveryViewer::Person(viewer.public_id.clone()),
        Caller::Viewer { .. } | Caller::Anonymous => DeliveryViewer::Visitor,
    }
}

/// Whose agents' handles a caller's thread views name (spec §13: other
/// people's agents by name and state only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handles {
    /// Every agent's: the owner by hand or in a browser.
    All,
    /// The agents of this person (`None`: the owner's agents).
    Of(Option<String>),
    /// No agent's: LAN viewers and anonymous callers.
    None,
}

impl Handles {
    /// The handles `caller` may be shown.
    pub fn of(caller: &crate::auth::Caller) -> Handles {
        use crate::auth::Caller;
        match caller {
            Caller::Owner { .. } => Handles::All,
            Caller::OwnerAgent { .. } => Handles::Of(None),
            Caller::PersonAgent { person, .. } => Handles::Of(person.clone()),
            Caller::Person { viewer } => Handles::Of(Some(viewer.public_id.clone())),
            Caller::Viewer { .. } | Caller::Anonymous => Handles::None,
        }
    }

    fn shows(&self, person: &Option<String>) -> bool {
        match self {
            Handles::All => true,
            Handles::Of(p) => p == person,
            Handles::None => false,
        }
    }
}

impl VisFor {
    /// What `caller` may see: agent channels are filtered for their
    /// reader; people are not (§8.5).
    pub fn of(caller: &crate::auth::Caller, s: &AppState) -> VisFor {
        VisFor {
            reader: caller.reader(),
            generation: s.vis_gen.current(),
            agent: caller
                .is_agent()
                .then(|| caller.session().map(str::to_string)),
            handles: Handles::of(caller),
            viewer: delivery_viewer(caller),
            clip_links: caller.is_agent().then(|| ClipLinks::of(s)).flatten(),
        }
    }

    /// The view, read from `st`.
    ///
    /// # Errors
    /// The store's.
    pub fn build(&self, st: &Store) -> clax_core::Result<Vis> {
        Ok(Vis {
            vis: match &self.reader {
                Some(r) => Some(Visibility::for_reader(st, r, self.generation)?),
                None => None,
            },
            agent: self.agent.clone(),
            handles: self.handles.clone(),
            clip_links: self.clip_links.clone(),
            viewer: match &self.viewer {
                // While nothing can be held, every agent reads every
                // thread's state, as before people existed (§19).
                DeliveryViewer::Agent { session, .. } => DeliveryViewer::Agent {
                    session: session.clone(),
                    every: st.owner_sees_all()?,
                },
                v => v.clone(),
            },
        })
    }
}

/// What an agent channel may see of the threads a route returns, field by
/// field (spec §8.2, §8.3, M10); for people ([`VisFor`] without a reader)
/// and for a view that sees everything, the views as they are.
pub struct Vis {
    vis: Option<Visibility>,
    /// For an agent channel, its session, if it names one.
    agent: Option<Option<String>>,
    handles: Handles,
    viewer: DeliveryViewer,
    clip_links: Option<ClipLinks>,
}

impl Vis {
    /// The filtering view, unless everything is visible.
    fn filtering(&self) -> Option<&Visibility> {
        self.vis.as_ref().filter(|v| !v.is_all())
    }

    /// Viewer views `people` (an array of objects with the viewer's public
    /// ID at `id_key` and its `display_name`) as the caller reads them: for
    /// an agent channel, each viewer who is not connected named `<name>
    /// (not connected)` (spec §9.6, §19 item 7); for people, as they are.
    pub fn mark_viewers(&self, people: &mut Value, id_key: &str) {
        if let Some(vis) = &self.vis {
            mark_viewers(vis, people, id_key);
        }
    }

    /// Whether anything may be withheld from the reader.
    pub fn filters(&self) -> bool {
        self.filtering().is_some()
    }

    /// [`thread_views`] of `threads`, each field the reader may not see
    /// replaced by its placeholder ([`Withhold::thread`]). For an agent,
    /// a thread another live agent owns says `taken_by: <its name>` (spec
    /// §12; names are not filtered, §21.9).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_threads(
        &self,
        st: &Store,
        threads: &[Thread],
        codex_push: bool,
        with_path: bool,
    ) -> clax_core::Result<Vec<Value>> {
        let (mut views, senders, owners) =
            views_and_senders(st, threads, codex_push, with_path, &self.viewer)?;
        for (view, owner) in views.iter_mut().zip(&owners) {
            if let Some(o) = owner
                && self.handles.shows(&o.person)
            {
                view["owner"]["handle"] = json!(o.handle);
            }
        }
        if let Some(me) = &self.agent {
            // A person's name in an agent's payload is the payload-safe one.
            for view in views.iter_mut() {
                if let Some(n) = view["owner"]["person_name"].as_str().map(str::to_string) {
                    view["owner"]["person_name"] = json!(clax_core::feedback::display_name(&n));
                }
            }
            for (view, owner) in views.iter_mut().zip(&owners) {
                if let Some(o) = owner
                    && o.live
                    && me.as_deref() != Some(o.session_id.as_str())
                {
                    view["taken_by"] = json!(o.display());
                }
            }
        }
        if let Some(vis) = self.filtering() {
            let mut w = Withhold::new(st, vis);
            for ((t, view), senders) in threads.iter().zip(&mut views).zip(&senders) {
                w.thread(t, view, senders)?;
            }
        }
        if let Some(vis) = &self.vis {
            for ((t, view), senders) in threads.iter().zip(&mut views).zip(&senders) {
                mark_thread(vis, t, view, senders);
            }
        }
        // A clip the reader is shown (its `clip_url` survived the filter)
        // is named at its keyed address too, but for a live page's.
        if let Some(links) = &self.clip_links {
            for (t, view) in threads.iter().zip(&mut views) {
                if !view["clip_url"].is_null()
                    && let Some(link) = links.link(&t.artifact_id, &t.id)
                {
                    view["clip_link"] = json!(link);
                }
            }
        }
        Ok(views)
    }

    /// [`Vis::filter_threads`] of one thread.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_thread(
        &self,
        st: &Store,
        t: &Thread,
        codex_push: bool,
        with_path: bool,
    ) -> clax_core::Result<Value> {
        Ok(self
            .filter_threads(st, std::slice::from_ref(t), codex_push, with_path)?
            .pop()
            .expect("one view per thread"))
    }

    /// Comment `c` of a thread of artifact `aid`, its body withheld and
    /// its author named as in its thread's view.
    pub fn filter_comment(&self, aid: &str, c: &Comment) -> Value {
        let mut v = json!(c);
        if let Some(vis) = &self.vis {
            mark_comment(vis, c, &mut v);
        }
        if let Some(vis) = self.filtering()
            && !vis.sees(&visibility::authors_of_comment(c), aid, Some(&c.thread_id))
        {
            placeholder::comment(&mut v);
        }
        v
    }

    /// The authors withholding thread `t`'s clip from the reader, `None`
    /// when it may see it (or the thread has none).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_clip(
        &self,
        st: &Store,
        t: &Thread,
    ) -> clax_core::Result<Option<Vec<WithheldAuthor>>> {
        let Some(vis) = self.filtering() else {
            return Ok(None);
        };
        if !t.has_clip {
            return Ok(None);
        }
        let mut w = Withhold::new(st, vis);
        let withheld = match w.version(&t.artifact_id, t.version_n)? {
            Some(v) => vis.withheld_authors(
                &visibility::authors_of_clip(t, &v),
                &t.artifact_id,
                Some(&t.id),
            ),
            None => Vec::new(),
        };
        let unknown = w.version(&t.artifact_id, t.version_n)?.is_none();
        if withheld.is_empty() && !unknown {
            return Ok(None);
        }
        Ok(Some(w.named(&withheld)?))
    }

    /// An entry of the all-threads listing for artifact `a`, whose
    /// `files` are `version`'s: its `title` null with `withheld: {authors}`
    /// when the artifact's text is withheld, and each file the reader may
    /// not see left out of `files`, counted in `files_withheld`.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_listing_entry(
        &self,
        st: &Store,
        a: &Artifact,
        version: Option<&Version>,
        entry: &mut Value,
    ) -> clax_core::Result<()> {
        let Some(vis) = self.filtering() else {
            return Ok(());
        };
        let mut w = Withhold::new(st, vis);
        let text = vis.withheld_authors(
            &visibility::authors_of_artifact_field(a, "title"),
            &a.id,
            None,
        );
        if !text.is_empty() {
            entry["title"] = Value::Null;
            entry["withheld"] = json!({"authors": w.named(&text)?});
        }
        let mut hidden = Vec::new();
        if let Some(v) = version {
            for path in v.files.keys() {
                if !vis.sees(&visibility::authors_of_file(v, path), &a.id, None) {
                    hidden.push(path.clone());
                }
            }
        }
        if let Some(files) = entry.get_mut("files").and_then(Value::as_array_mut) {
            files.retain(|f| !hidden.iter().any(|h| f.as_str() == Some(h)));
        }
        entry["files_withheld"] = json!(hidden.len());
        Ok(())
    }

    /// A question's view `view` (crate::views::question::view) as its asking
    /// agent sees it: the question, its answers and its agent are the
    /// agent's person's own (spec §8.2); its artifact's title is the
    /// title's authors', null with `withheld: {authors}` beside it in
    /// `artifact` when the reader may not see it.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_question(&self, st: &Store, view: &mut Value) -> clax_core::Result<()> {
        let Some(vis) = self.filtering() else {
            return Ok(());
        };
        let Some(aid) = view["artifact"]["id"].as_str() else {
            return Ok(());
        };
        let Some(a) = st.get_artifact(&ArtifactId::parse(aid)?)? else {
            return Ok(());
        };
        let withheld = vis.withheld_authors(
            &visibility::authors_of_artifact_field(&a, "title"),
            &a.id,
            None,
        );
        if !withheld.is_empty() {
            let named = Withhold::new(st, vis).named(&withheld)?;
            view["artifact"]["title"] = Value::Null;
            view["artifact"]["withheld"] = json!({"authors": named});
        }
        Ok(())
    }

    /// An approval's view `view` ([`crate::views::approval::approval_view`]) as
    /// the reader may see it: its page's title is the title's authors',
    /// null when the reader may not see it (spec §7.5, §8.2).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_approval(&self, st: &Store, view: &mut Value) -> clax_core::Result<()> {
        let Some(vis) = self.filtering() else {
            return Ok(());
        };
        if view["artifact_title"].is_null() {
            return Ok(());
        }
        let Some(aid) = view["artifact_id"].as_str() else {
            return Ok(());
        };
        let hidden = match st.get_artifact(&ArtifactId::parse(aid)?)? {
            Some(a) => !vis
                .withheld_authors(
                    &visibility::authors_of_artifact_field(&a, "title"),
                    &a.id,
                    None,
                )
                .is_empty(),
            None => true,
        };
        if hidden {
            view["artifact_title"] = Value::Null;
        }
        Ok(())
    }

    /// An artifact's view `view` (its JSON, with or without the fields of
    /// `with_owner`): each of its text fields (title, description, icon,
    /// `capabilities`) the reader may not see null, with `withheld:
    /// {authors}` naming whose they are; and, where the view embeds
    /// `working`, `working`'s records ([`Vis::filter_working`]) in its
    /// place.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_artifact(
        &self,
        st: &Store,
        a: &Artifact,
        working: &[WorkingView],
        view: &mut Value,
    ) -> clax_core::Result<()> {
        if let Some(vis) = &self.vis {
            mark_participants(vis, view);
        }
        let Some(vis) = self.filtering() else {
            return Ok(());
        };
        let mut w = Withhold::new(st, vis);
        w.artifact(a, view)?;
        if view.get("working").is_some() {
            view["working"] = json!(w.working(&a.id, working)?);
        }
        Ok(())
    }

    /// `records` of artifact `aid` as views list them, each message the
    /// reader may not see (it is its session's person's) null with
    /// `withheld: true`.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_working(
        &self,
        st: &Store,
        aid: &str,
        records: &[WorkingView],
    ) -> clax_core::Result<Vec<Value>> {
        match self.filtering() {
            Some(vis) => Withhold::new(st, vis).working(aid, records),
            None => Ok(records.iter().map(|r| json!(r)).collect()),
        }
    }

    /// The working roster's `records`, each message the reader may not see
    /// null with `withheld: true`.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_roster(
        &self,
        st: &Store,
        records: &[SessionWorking],
    ) -> clax_core::Result<Vec<Value>> {
        let Some(vis) = self.filtering() else {
            return Ok(records.iter().map(|r| json!(r)).collect());
        };
        let mut w = Withhold::new(st, vis);
        records
            .iter()
            .map(|r| {
                let mut v = json!(r);
                if w.hides_working(&r.artifact_id, &r.session_id)? {
                    placeholder::working(&mut v);
                }
                Ok(v)
            })
            .collect()
    }

    /// Version `v`'s view `view`: its label and note null when the reader
    /// may not see them, and each file it may not see left out of `files`,
    /// counted in `files_withheld` (with the version's `content_sha256`
    /// null when any is).
    pub fn filter_version(&self, v: &Version, view: &mut Value) {
        let Some(vis) = self.filtering() else {
            return;
        };
        if !vis.sees(&visibility::authors_of_version(v), &v.artifact_id, None) {
            placeholder::version_text(view);
        }
        let hidden = withheld_files(vis, v);
        placeholder::version_files(view, &hidden);
    }

    /// A view of version `v`'s `files` (`{files, version}`): each file the
    /// reader may not see left out, counted in `files_withheld`.
    pub fn filter_files(&self, v: &Version, view: &mut Value) {
        if let Some(vis) = self.filtering() {
            placeholder::version_files(view, &withheld_files(vis, v));
        }
    }

    /// A publish's answer: artifact `a` and its new version `v` as JSON,
    /// filtered ([`Vis::filter_artifact`], [`Vis::filter_version`]): the
    /// publisher's reader sees no file it carried forward from someone it
    /// has not approved (spec §7.5, M11).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_published(
        &self,
        st: &Store,
        a: &Artifact,
        v: &Version,
    ) -> clax_core::Result<(Value, Value)> {
        let mut artifact = json!(a);
        self.filter_artifact(st, a, &[], &mut artifact)?;
        let mut version = json!(v);
        self.filter_version(v, &mut version);
        Ok((artifact, version))
    }

    /// The paths of version `v`'s files the reader may not see.
    pub fn filter_hidden_files(&self, v: &Version) -> Vec<String> {
        match self.filtering() {
            Some(vis) => withheld_files(vis, v)
                .into_iter()
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        }
    }

    /// [`Vis::filter_version`] of each of `versions`, as views.
    pub fn filter_versions(&self, versions: &[Version]) -> Vec<Value> {
        versions
            .iter()
            .map(|v| {
                let mut view = json!(v);
                self.filter_version(v, &mut view);
                view
            })
            .collect()
    }

    /// The authors withholding file `path` of version `v` from the reader,
    /// `None` when it may read it. So that the answer tells nothing of a
    /// name the reader guessed, a file that is withheld and one the
    /// version lacks answer alike while the version withholds any file:
    /// with every author of its withheld files. A version that withholds
    /// nothing withholds nothing here either (a missing file is then
    /// missing).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_file_content(
        &self,
        st: &Store,
        v: &Version,
        path: &str,
    ) -> clax_core::Result<Option<Vec<WithheldAuthor>>> {
        let Some(vis) = self.filtering() else {
            return Ok(None);
        };
        let hidden = withheld_files(vis, v);
        if hidden.is_empty() || (v.files.contains_key(path) && !hidden.contains(&path)) {
            return Ok(None);
        }
        let mut authors = Vec::new();
        for p in hidden {
            for a in vis.withheld_authors(&visibility::authors_of_file(v, p), &v.artifact_id, None)
            {
                if !authors.contains(&a) {
                    authors.push(a);
                }
            }
        }
        Ok(Some(Withhold::new(st, vis).named(&authors)?))
    }

    /// Whether an anchor on `file` of version `v` must answer as one on a
    /// file the version lacks: the reader may not see the file's name.
    /// Every version has its `index.html`, so an anchor there tells
    /// nothing and is never refused.
    pub fn filter_anchor_file(&self, v: &Version, file: &str) -> bool {
        let Some(vis) = self.filtering() else {
            return false;
        };
        file != clax_core::publish::INDEX
            && v.files.contains_key(file)
            && !vis.sees(&visibility::authors_of_file(v, file), &v.artifact_id, None)
    }

    /// The view a store read evaluates with, when anything may be
    /// withheld: a query matches and orders only what it lets through
    /// ([`Store::doc_query_visible`]).
    pub fn view(&self) -> Option<&Visibility> {
        self.filtering()
    }

    /// Document `doc` of artifact `aid`, whose authors are `authors`: its
    /// view, or when the reader may not see it the placeholder of a
    /// document read by its path, `{doc_id, version, withheld: {authors}}`
    /// without its data (spec §8.3).
    ///
    /// # Errors
    /// The store's.
    pub fn filter_doc(
        &self,
        st: &Store,
        aid: &str,
        doc: &Doc,
        authors: &[Author],
    ) -> clax_core::Result<Value> {
        if let Some(vis) = self.filtering() {
            let withheld = vis.withheld_authors(authors, aid, None);
            if !withheld.is_empty() {
                let named = Withhold::new(st, vis).named(&withheld)?;
                return Ok(placeholder::document(&doc.id, doc.version, &named));
            }
        }
        Ok(json!(doc))
    }

    /// The document a write of artifact `aid` left, as
    /// [`Vis::filter_doc`] shows it; `None` after a delete.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_written(
        &self,
        st: &Store,
        aid: &str,
        w: &Written,
    ) -> clax_core::Result<Option<Value>> {
        w.doc
            .as_ref()
            .map(|d| self.filter_doc(st, aid, d, &w.authors))
            .transpose()
    }

    /// A query's page as the list route answers it: `{docs, next_cursor}`,
    /// with `withheld_count` (the collection's documents left out,
    /// whatever the filter) when the reader may not see every document.
    /// The store matched, ordered and paged only what the reader may see.
    pub fn filter_doc_page(&self, page: DocPage) -> Value {
        let mut v = json!({"docs": page.docs, "next_cursor": page.next_cursor});
        if let Some(n) = page.withheld {
            v["withheld_count"] = json!(n);
        }
        v
    }

    /// A batch's results, `ops` its operations' names in order: `{op,
    /// path, version, deleted}` each, which hold no document field (the
    /// path is the caller's own), so nothing is withheld.
    pub fn filter_batch(&self, written: &[Written], ops: Vec<String>) -> Vec<Value> {
        written
            .iter()
            .zip(ops)
            .map(|(w, op)| json!({"op": op, "path": w.path, "version": w.doc.as_ref().map(|d| d.version), "deleted": w.deleted}))
            .collect()
    }

    /// A lease's answer: whether it was granted, the document's version,
    /// the lease's expiry and, on a grant, the caller's own holder. It
    /// holds no document field, so nothing is withheld.
    pub fn filter_acquired(&self, a: &Acquired) -> Value {
        json!(a)
    }

    /// `authors`, withheld from the reader, as views name them.
    ///
    /// # Errors
    /// The store's.
    pub fn filter_authors(
        &self,
        st: &Store,
        authors: &[Author],
    ) -> clax_core::Result<Vec<WithheldAuthor>> {
        match self.filtering() {
            Some(vis) => Withhold::new(st, vis).named(authors),
            None => Ok(Vec::new()),
        }
    }

    /// `assets` as views list them, each the reader may not see with its
    /// name and what its bytes decide null, and `withheld: true`.
    pub fn filter_assets(&self, assets: &[Asset]) -> Vec<Value> {
        assets
            .iter()
            .map(|a| {
                let mut v = json!(a);
                if let Some(vis) = self.filtering()
                    && !vis.sees(&visibility::authors_of_asset(a), &a.artifact_id, None)
                {
                    placeholder::asset(&mut v);
                }
                v
            })
            .collect()
    }
}

/// The paths of version `v`'s files `vis` withholds, in order.
fn withheld_files<'a>(vis: &Visibility, v: &'a Version) -> Vec<&'a str> {
    v.files
        .keys()
        .filter(|p| !vis.sees(&visibility::authors_of_file(v, p), &v.artifact_id, None))
        .map(String::as_str)
        .collect()
}

/// Applies one reader's view to thread views, reading what it needs once:
/// the versions anchors are on, the names of withheld authors.
struct Withhold<'a> {
    st: &'a Store,
    vis: &'a Visibility,
    versions: HashMap<(String, u32), Option<Version>>,
    names: HashMap<Author, WithheldAuthor>,
    /// Each session's person, as read (`None` inside: the owner's).
    persons: HashMap<String, Option<Option<String>>>,
}

impl<'a> Withhold<'a> {
    fn new(st: &'a Store, vis: &'a Visibility) -> Withhold<'a> {
        Withhold {
            st,
            vis,
            versions: HashMap::new(),
            names: HashMap::new(),
            persons: HashMap::new(),
        }
    }

    /// Whether the reader may not see the message of a working record of
    /// session `sid` on `aid`: its session's person's words. A session
    /// that is gone names no one: its message is withheld.
    fn hides_working(&mut self, aid: &str, sid: &str) -> clax_core::Result<bool> {
        if !self.persons.contains_key(sid) {
            let p = self.st.get_session(sid)?.map(|s| s.person);
            self.persons.insert(sid.to_string(), p);
        }
        Ok(match &self.persons[sid] {
            Some(person) => !self.vis.sees(
                &visibility::authors_of_working(person.as_deref()),
                aid,
                None,
            ),
            None => true,
        })
    }

    /// `records` of `aid` as views, each message withheld as
    /// [`Withhold::hides_working`] says.
    fn working(&mut self, aid: &str, records: &[WorkingView]) -> clax_core::Result<Vec<Value>> {
        records
            .iter()
            .map(|r| {
                let mut v = json!(r);
                if self.hides_working(aid, &r.session_id)? {
                    placeholder::working(&mut v);
                }
                Ok(v)
            })
            .collect()
    }

    /// Artifact `a`'s view: each text field the reader may not see null,
    /// and `withheld: {authors}` when any is.
    fn artifact(&mut self, a: &Artifact, view: &mut Value) -> clax_core::Result<()> {
        let mut fields = Vec::new();
        let mut withheld: Vec<Author> = Vec::new();
        for f in clax_core::model::ARTIFACT_TEXT_FIELDS {
            let w = self.vis.withheld_authors(
                &visibility::authors_of_artifact_field(a, f),
                &a.id,
                None,
            );
            if !w.is_empty() {
                fields.push(f);
                for x in w {
                    if !withheld.contains(&x) {
                        withheld.push(x);
                    }
                }
            }
        }
        if !fields.is_empty() {
            let named = self.named(&withheld)?;
            placeholder::artifact(view, &fields, &named);
        }
        Ok(())
    }

    fn version(&mut self, aid: &str, n: u32) -> clax_core::Result<Option<Version>> {
        let key = (aid.to_string(), n);
        if !self.versions.contains_key(&key) {
            let v = self.st.get_version(&ArtifactId::parse(aid)?, n)?;
            self.versions.insert(key.clone(), v);
        }
        Ok(self.versions[&key].clone())
    }

    /// `authors` as views list them: public ID (none for anonymous
    /// viewers) and current display name.
    fn named(&mut self, authors: &[Author]) -> clax_core::Result<Vec<WithheldAuthor>> {
        let mut out = Vec::with_capacity(authors.len());
        for a in authors {
            if !self.names.contains_key(a) {
                let viewer = match a {
                    Author::Owner => self.st.owner()?,
                    Author::Person(p) => self.st.viewer_by_public_id(p)?,
                    Author::Unconnected | Author::System => None,
                };
                let named = WithheldAuthor {
                    public_id: match a {
                        Author::Person(p) => Some(p.clone()),
                        _ => viewer.as_ref().map(|v| v.public_id.clone()),
                    },
                    name: self.vis.label(
                        &clax_core::feedback::person_name(
                            viewer.as_ref().and_then(|v| v.display_name.as_deref()),
                            *a == Author::Owner,
                        ),
                        a,
                    ),
                };
                self.names.insert(a.clone(), named);
            }
            out.push(self.names[a].clone());
        }
        Ok(out)
    }

    /// Where the reader's approval of `authors` stands at thread `tid` of
    /// artifact `aid`: an open request for any of them (at the thread or
    /// the whole artifact) is `requested`, else a declined one `declined`.
    fn approval(
        &mut self,
        aid: &str,
        tid: &str,
        authors: &[Author],
    ) -> clax_core::Result<ApprovalState> {
        let approver = match self.vis.reader() {
            Some(Reader::Person(p)) => Some(p.clone()),
            _ => None,
        };
        let mut keys = Vec::new();
        for a in authors {
            match a {
                Author::Owner => {
                    if let Some(o) = self.st.owner()? {
                        keys.push(o.public_id);
                    }
                }
                Author::Person(p) => keys.push(p.clone()),
                Author::Unconnected => keys.push(UNCONNECTED.to_string()),
                Author::System => {}
            }
        }
        self.st.request_state(approver.as_deref(), aid, tid, &keys)
    }

    /// Thread `t`'s view, `senders` its batches' senders in `sends` order:
    /// each comment body, the anchor, the clip, `resolved_by` and each
    /// batch note the reader may not see is replaced by its placeholder
    /// (§8.3), and when any is, the thread gains `withheld: {count (its
    /// comments withheld), authors, approval}`.
    fn thread(
        &mut self,
        t: &Thread,
        view: &mut Value,
        senders: &[Option<String>],
    ) -> clax_core::Result<()> {
        let vis = self.vis;
        let (aid, tid) = (t.artifact_id.as_str(), Some(t.id.as_str()));
        let mut withheld: Vec<Author> = Vec::new();
        let mut any = false;
        let mut hides = |authors: &[Author]| {
            let w = vis.withheld_authors(authors, aid, tid);
            for a in &w {
                if !withheld.contains(a) {
                    withheld.push(a.clone());
                }
            }
            !w.is_empty()
        };
        let mut count = 0;
        for (i, c) in t.comments.iter().enumerate() {
            if hides(&visibility::authors_of_comment(c)) {
                placeholder::comment(&mut view["comments"][i]);
                count += 1;
                any = true;
            }
        }
        // A version that is gone names no authors: withhold what it held.
        let version = self.version(aid, t.version_n)?;
        let (anchor, clip) = match &version {
            Some(v) => (
                hides(&visibility::authors_of_anchor(t, v)),
                t.has_clip && hides(&visibility::authors_of_clip(t, v)),
            ),
            None => (!t.anchor.is_whole(), t.has_clip),
        };
        if anchor {
            placeholder::anchor(view);
            any = true;
        }
        if clip {
            placeholder::clip(view);
            any = true;
        }
        if let Some(by) = &t.resolved_by
            && hides(&visibility::authors_of_resolved_by(
                by,
                t.resolved_by_person.as_deref(),
            ))
        {
            placeholder::resolved_by(view);
            any = true;
        }
        for (i, sender) in senders.iter().enumerate() {
            let note = &view["sends"][i]["note"];
            if !note.is_null() && hides(&visibility::authors_of_batch_note(sender.as_deref())) {
                placeholder::batch_note(&mut view["sends"][i]);
                any = true;
            }
        }
        if any {
            let authors = self.named(&withheld)?;
            let approval = self.approval(aid, &t.id, &withheld)?;
            placeholder::thread_summary(view, count, &authors, approval);
        }
        Ok(())
    }
}

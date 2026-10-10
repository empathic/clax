//! Whose words may reach an agent (spec 2026-10-08-multi-agent-multi-person
//! §8): the reader an agent reads for, the authors of each field (§8.2),
//! the filter that answers "may an agent of reader R see a field whose
//! effective authors are A₁…Aₙ, at artifact Y (thread T)?" (§8.1), and the
//! placeholders that replace a withheld field (§8.3).
//!
//! The filter runs for agent channels only (§8.5); people's browsers and
//! the owner's CLI by hand see everything ([`Visibility::all`]).

use crate::Result;
use crate::model::{Artifact, Asset, Comment, Thread, Version, VersionAuthors};
use crate::store::Store;
use crate::store::approvals::{OWNER, UNCONNECTED};
use rusqlite::params;
use std::collections::HashSet;

/// The person an agent channel reads for (spec §6.1): the owner, for the
/// owner's agents and for agents of a grant of the owner's row; else the
/// connected person, by public ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Reader {
    Owner,
    Person(String),
}

/// One effective author of a field (§8.2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Author {
    /// The owner: `person` NULL, `*owner`, or the owner's public ID.
    Owner,
    /// A viewer by public ID: a connected (or revoked) person, or a LAN
    /// viewer who is not connected, whom an approval of `*unconnected`
    /// covers.
    Person(String),
    /// Every viewer with no public ID: anonymous callers, and comments
    /// from before viewers had public IDs.
    Unconnected,
    /// Clax itself: placeholders and notes the daemon writes.
    System,
}

impl Author {
    /// The author a stored person value names: `None` and `*owner` are
    /// the owner, `*unconnected` the class, anything else a public ID.
    pub fn of_token(token: Option<&str>) -> Author {
        match token {
            None | Some(OWNER) => Author::Owner,
            Some(UNCONNECTED) => Author::Unconnected,
            Some(p) => Author::Person(p.to_string()),
        }
    }

    /// The author of a viewer's words: its public ID, or
    /// [`Author::Unconnected`] without one.
    pub fn of_viewer(public_id: Option<&str>) -> Author {
        match public_id {
            Some(p) => Author::of_token(Some(p)),
            None => Author::Unconnected,
        }
    }

    /// How a store column holds this author (`doc_authors.author`,
    /// `comments.page_person`, `versions.file_authors_json`); `None` for
    /// [`Author::System`], which is never stored.
    pub fn token(&self) -> Option<&str> {
        match self {
            Author::Owner => Some(OWNER),
            Author::Person(p) => Some(p),
            Author::Unconnected => Some(UNCONNECTED),
            Author::System => None,
        }
    }
}

/// `authors` with the owner's public ID (`owner`) as [`Author::Owner`] and
/// each author once, in first-seen order.
pub fn normalise(authors: &[Author], owner: Option<&str>) -> Vec<Author> {
    let mut out: Vec<Author> = Vec::with_capacity(authors.len());
    for a in authors {
        let a = match a {
            Author::Person(p) if Some(p.as_str()) == owner => Author::Owner,
            a => a.clone(),
        };
        push(&mut out, a);
    }
    out
}

fn push(out: &mut Vec<Author>, a: Author) {
    if !out.contains(&a) {
        out.push(a);
    }
}

/// The authors of a version's own words: its publisher, and for a version
/// a page published (`by_page`) the page's publisher fixed at that write.
fn publisher(person: Option<&str>, by_page: bool, page_person: Option<&str>) -> Vec<Author> {
    let mut out = vec![Author::of_token(person)];
    if by_page {
        push(&mut out, Author::of_token(page_person));
    }
    out
}

/// A comment's body (§8.2): an agent comment's is its session's person;
/// a viewer comment's is its writer (anonymous, or from before public
/// IDs: [`Author::Unconnected`]), and for one a page wrote (`via_page`)
/// also the page's publisher at write time (`page_person`; none recorded:
/// the owner).
pub fn authors_of_comment(c: &Comment) -> Vec<Author> {
    if c.author_kind == crate::store::threads::AUTHOR_AGENT {
        return vec![Author::of_token(c.agent_person.as_deref())];
    }
    let mut out = vec![Author::of_viewer(c.author_public_id.as_deref())];
    if c.via_page {
        push(&mut out, Author::of_token(c.page_person.as_deref()));
    }
    out
}

/// A file of `version` (its name and bytes): its authors in
/// `file_authors_json`, fixed when it was written or carried forward; a
/// file with none recorded is the version's publisher's.
pub fn authors_of_file(version: &Version, path: &str) -> Vec<Author> {
    file_authors(&version.authors, path)
}

/// [`authors_of_file`] of a version's authorship columns.
pub fn file_authors(a: &VersionAuthors, path: &str) -> Vec<Author> {
    match a.files.as_ref().and_then(|f| f.get(path)) {
        Some(tokens) if !tokens.is_empty() => {
            let mut out = Vec::new();
            for t in tokens {
                push(&mut out, Author::of_token(t.as_deref()));
            }
            out
        }
        _ => publisher(a.person.as_deref(), a.by_page, a.page_person.as_deref()),
    }
}

/// A thread's anchor (quote, prefix, suffix, selector): the authors of the
/// anchored file in the thread's version, `version` (the version's
/// publisher when it has no such file).
pub fn authors_of_anchor(thread: &Thread, version: &Version) -> Vec<Author> {
    authors_of_file(version, &thread.anchor.file)
}

/// A thread's clip: its first commenter and its anchor's authors.
pub fn authors_of_clip(thread: &Thread, version: &Version) -> Vec<Author> {
    let mut out = thread
        .comments
        .first()
        .map(authors_of_comment)
        .unwrap_or_default();
    for a in authors_of_anchor(thread, version) {
        push(&mut out, a);
    }
    out
}

/// A thread's `resolved_by` text: `viewer:<public ID>` is that viewer;
/// `viewer:anonymous`, and `agent:<harness>` (which names no person), are
/// [`Author::Unconnected`].
pub fn authors_of_resolved_by(resolved_by: &str) -> Vec<Author> {
    match resolved_by.strip_prefix("viewer:") {
        Some(p) if crate::is_public_id(p) => vec![Author::Person(p.to_string())],
        _ => vec![Author::Unconnected],
    }
}

/// A batch's note: its sender, by public ID (none: [`Author::Unconnected`]).
pub fn authors_of_batch_note(sender_public_id: Option<&str>) -> Vec<Author> {
    vec![Author::of_viewer(sender_public_id)]
}

/// A version's label and note: its publisher, and for a version a page
/// published, the page's publisher fixed at that write. Its files have
/// authors of their own ([`authors_of_file`]).
pub fn authors_of_version(version: &Version) -> Vec<Author> {
    let a = &version.authors;
    publisher(a.person.as_deref(), a.by_page, a.page_person.as_deref())
}

/// An artifact's title, description, icon and `capabilities`: its creator
/// and its current version's publisher (with that version's page
/// publisher when a page published it).
pub fn authors_of_artifact_text(a: &Artifact) -> Vec<Author> {
    let mut out = vec![Author::of_token(a.authors.creator.as_deref())];
    let c = &a.authors.current;
    for p in publisher(c.person.as_deref(), c.by_page, c.page_person.as_deref()) {
        push(&mut out, p);
    }
    out
}

/// An asset's name and bytes: its uploader.
pub fn authors_of_asset(a: &Asset) -> Vec<Author> {
    vec![Author::of_token(a.person.as_deref())]
}

/// A document's fields: every stored author (`doc_authors.author`), or the
/// owner when it has none.
pub fn authors_of_doc(tokens: &[String]) -> Vec<Author> {
    if tokens.is_empty() {
        return vec![Author::Owner];
    }
    let mut out = Vec::new();
    for t in tokens {
        push(&mut out, Author::of_token(Some(t)));
    }
    out
}

/// A working record's message: its session's person (`None`: the owner).
pub fn authors_of_working(session_person: Option<&str>) -> Vec<Author> {
    vec![Author::of_token(session_person)]
}

/// What a reader's approvals let through, by author key (a public ID,
/// [`OWNER`] or [`UNCONNECTED`]).
#[derive(Clone, Debug, Default)]
struct Approved {
    /// The owner's public ID, read as [`Author::Owner`].
    owner: Option<String>,
    /// Every person's public ID (connected or revoked), loaded only when
    /// an approval of `*unconnected` makes the difference matter.
    people: HashSet<String>,
    everywhere: HashSet<String>,
    artifacts: HashSet<(String, String)>,
    threads: HashSet<(String, String)>,
}

impl Approved {
    fn covers(&self, key: &str, artifact: &str, thread: Option<&str>) -> bool {
        self.everywhere.contains(key)
            || self
                .artifacts
                .contains(&(key.to_string(), artifact.to_string()))
            || thread.is_some_and(|t| self.threads.contains(&(key.to_string(), t.to_string())))
    }
}

#[derive(Clone, Debug)]
enum View {
    All,
    Filtered(Box<Approved>),
}

/// What one agent channel may see (§8.1): built for a reader at a
/// generation of the store's visibility (M17), rebuilt when it changes.
#[derive(Clone, Debug)]
pub struct Visibility {
    generation: u64,
    reader: Option<Reader>,
    view: View,
}

impl Visibility {
    /// Sees everything, and never goes stale: for callers that are not
    /// agent channels (§8.5).
    pub fn all() -> Visibility {
        Visibility {
            generation: 0,
            reader: None,
            view: View::All,
        }
    }

    /// What agents of `reader` may see, as of visibility `generation`
    /// (read it before calling, so a change committed meanwhile leaves the
    /// result stale, to be refreshed, rather than unnoticed). Reads the
    /// reader's live approvals in one indexed query. For the owner while
    /// no viewer is a person and the `*unconnected` approval is live, sees
    /// everything at the cost of one atomic load (§8.7).
    pub fn for_reader(st: &Store, reader: &Reader, generation: u64) -> Result<Visibility> {
        let reader = Some(reader.clone());
        if reader == Some(Reader::Owner) && st.owner_sees_all()? {
            return Ok(Visibility {
                generation,
                reader,
                view: View::All,
            });
        }
        let approver = match &reader {
            Some(Reader::Person(p)) => p.as_str(),
            _ => "",
        };
        let approved = st.with_read(|c| {
            let mut a = Approved {
                owner: c
                    .prepare_cached("SELECT public_id FROM viewers WHERE owner = 1")?
                    .query_row([], |r| r.get(0))
                    .ok(),
                ..Approved::default()
            };
            let mut stmt = c.prepare_cached(APPROVALS_OF)?;
            let rows = stmt.query_map(params![approver], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })?;
            let mut unconnected = false;
            for row in rows {
                let (author, scope, artifact, thread) = row?;
                let key = if a.owner.as_deref() == Some(author.as_str()) {
                    OWNER.to_string()
                } else {
                    author
                };
                unconnected |= key == UNCONNECTED;
                match (scope.as_str(), artifact, thread) {
                    ("everywhere", _, _) => {
                        a.everywhere.insert(key);
                    }
                    ("artifact", Some(aid), _) => {
                        a.artifacts.insert((key, aid));
                    }
                    ("thread", _, Some(tid)) => {
                        a.threads.insert((key, tid));
                    }
                    _ => {}
                }
            }
            if unconnected {
                let mut stmt =
                    c.prepare_cached("SELECT public_id FROM viewers WHERE person = 1")?;
                for p in stmt.query_map([], |r| r.get::<_, String>(0))? {
                    a.people.insert(p?);
                }
            }
            Ok(a)
        })?;
        Ok(Visibility {
            generation,
            reader,
            view: View::Filtered(Box::new(approved)),
        })
    }

    /// Rebuilds this view for its reader when `current` is not the
    /// generation it was built at. A view from [`Visibility::all`] stays.
    pub fn refresh_if_stale(&mut self, st: &Store, current: u64) -> Result<()> {
        if current == self.generation {
            return Ok(());
        }
        if let Some(reader) = &self.reader {
            *self = Visibility::for_reader(st, reader, current)?;
        }
        Ok(())
    }

    /// Whether everything is visible.
    pub fn is_all(&self) -> bool {
        matches!(self.view, View::All)
    }

    /// The generation this view was built at.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The reader it was built for; `None` for [`Visibility::all`].
    pub fn reader(&self) -> Option<&Reader> {
        self.reader.as_ref()
    }

    /// `authors` normalised against this view's owner ([`normalise`]).
    pub fn normalise(&self, authors: &[Author]) -> Vec<Author> {
        match &self.view {
            View::All => normalise(authors, None),
            View::Filtered(a) => normalise(authors, a.owner.as_deref()),
        }
    }

    /// Whether the reader's agents may see a field written by `authors`
    /// at `artifact`, in `thread` when the field is in one (§8.1): yes
    /// when every author is the reader, is Clax itself, or is approved
    /// there (an author who is neither the owner nor a person also through
    /// an approval of `*unconnected`).
    pub fn sees(&self, authors: &[Author], artifact: &str, thread: Option<&str>) -> bool {
        authors.iter().all(|a| self.sees_one(a, artifact, thread))
    }

    /// The authors of `authors` this view withholds there, normalised.
    pub fn withheld_authors(
        &self,
        authors: &[Author],
        artifact: &str,
        thread: Option<&str>,
    ) -> Vec<Author> {
        self.normalise(authors)
            .into_iter()
            .filter(|a| !self.sees_one(a, artifact, thread))
            .collect()
    }

    fn sees_one(&self, author: &Author, artifact: &str, thread: Option<&str>) -> bool {
        let View::Filtered(a) = &self.view else {
            return true;
        };
        let reader = self.reader.as_ref();
        match author {
            Author::System => true,
            Author::Owner => reader == Some(&Reader::Owner) || a.covers(OWNER, artifact, thread),
            Author::Person(p) if a.owner.as_deref() == Some(p.as_str()) => {
                reader == Some(&Reader::Owner) || a.covers(OWNER, artifact, thread)
            }
            Author::Person(p) => {
                matches!(reader, Some(Reader::Person(r)) if r == p)
                    || a.covers(p, artifact, thread)
                    || (!a.people.contains(p) && a.covers(UNCONNECTED, artifact, thread))
            }
            Author::Unconnected => a.covers(UNCONNECTED, artifact, thread),
        }
    }
}

/// A reader's live approvals (`?1`: the approver's public ID, `''` for the
/// owner), through the index `approvals_by_approver`.
pub(crate) const APPROVALS_OF: &str = "SELECT author, scope, artifact_id, thread_id FROM approvals
     WHERE COALESCE(approver, '') = ?1 AND revoked_at IS NULL";

/// The placeholders that replace withheld fields in what an agent channel
/// is handed (§8.3), one function per shape. Each changes the JSON view of
/// its object in place, or builds the text, and never drops the object.
pub mod placeholder {
    use serde::Serialize;
    use serde_json::{Value, json};

    /// One withheld author as views list them.
    #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
    pub struct WithheldAuthor {
        /// `None` for anonymous viewers.
        pub public_id: Option<String>,
        pub name: String,
    }

    /// Where an approval for withheld content stands, for the reader.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "lowercase")]
    pub enum ApprovalState {
        None,
        Requested,
        Declined,
    }

    fn authors_json(authors: &[WithheldAuthor]) -> Value {
        serde_json::to_value(authors).expect("authors serialise")
    }

    /// A comment: `body` null and `withheld: true`; its ID, author name
    /// and person, and time stay.
    pub fn comment(comment: &mut Value) {
        comment["body"] = Value::Null;
        comment["withheld"] = json!(true);
    }

    /// A thread's anchor: `quote`, `prefix`, `suffix` and `selector` null
    /// in its `anchor`, and the thread marked `anchor_withheld: true`.
    pub fn anchor(thread: &mut Value) {
        if let Some(anchor) = thread.get_mut("anchor").and_then(Value::as_object_mut) {
            for k in ["quote", "prefix", "suffix", "selector"] {
                anchor.insert(k.into(), Value::Null);
            }
        }
        thread["anchor_withheld"] = json!(true);
    }

    /// A thread's clip: no `clip`, and `clip_withheld: true`.
    pub fn clip(thread: &mut Value) {
        if let Some(t) = thread.as_object_mut() {
            t.remove("clip");
        }
        thread["clip_withheld"] = json!(true);
    }

    /// A thread's `resolved_by`: null.
    pub fn resolved_by(thread: &mut Value) {
        thread["resolved_by"] = Value::Null;
    }

    /// A batch's note: null.
    pub fn batch_note(batch: &mut Value) {
        batch["note"] = Value::Null;
    }

    /// The summary a thread gains whenever any field of it is withheld:
    /// `withheld: {count, authors: [{public_id, name}], approval}`.
    pub fn thread_summary(
        thread: &mut Value,
        count: usize,
        authors: &[WithheldAuthor],
        approval: ApprovalState,
    ) {
        thread["withheld"] = json!({
            "count": count,
            "authors": authors_json(authors),
            "approval": approval,
        });
    }

    /// An artifact: `title`, `description`, `icon` and `capabilities`
    /// null, and `withheld: {authors}`.
    pub fn artifact(artifact: &mut Value, authors: &[WithheldAuthor]) {
        for k in ["title", "description", "icon", "capabilities"] {
            artifact[k] = Value::Null;
        }
        artifact["withheld"] = json!({"authors": authors_json(authors)});
    }

    /// A working record's message (embedded in an artifact or not):
    /// `message` null and `withheld: true`.
    pub fn working(record: &mut Value) {
        record["message"] = Value::Null;
        record["withheld"] = json!(true);
    }

    /// A version's label and note: null.
    pub fn version_text(version: &mut Value) {
        version["label"] = Value::Null;
        version["note"] = Value::Null;
    }

    /// A version's withheld files: each left out of `files`, and
    /// `files_withheld: n`, how many were.
    pub fn version_files(version: &mut Value, withheld: &[&str]) {
        let mut n = 0;
        if let Some(files) = version.get_mut("files").and_then(Value::as_object_mut) {
            for path in withheld {
                n += usize::from(files.remove(*path).is_some());
            }
        }
        version["files_withheld"] = json!(n);
    }

    /// A document read by its path: `{doc_id, version, withheld:
    /// {authors}}`, without its data.
    pub fn document(doc_id: &str, version: u64, authors: &[WithheldAuthor]) -> Value {
        json!({"doc_id": doc_id, "version": version, "withheld": {"authors": authors_json(authors)}})
    }

    /// An asset: `name` null and `withheld: true`.
    pub fn asset(asset: &mut Value) {
        asset["name"] = Value::Null;
        asset["withheld"] = json!(true);
    }

    /// An artifact's title where its text is withheld.
    pub fn title_by(name: &str) -> String {
        format!("an artifact by {name}")
    }

    /// The feedback line for `n` withheld comments by `author` in thread
    /// `thread_id` of the artifact titled `title` (`None` when its title is
    /// withheld: [`title_by`] `title_author`), saying whether the reader's
    /// person has been asked.
    pub fn withheld_comments_line(
        n: usize,
        author: &str,
        title: Option<&str>,
        title_author: &str,
        thread_id: &str,
        asked: bool,
    ) -> String {
        let title = match title {
            Some(t) => t.to_string(),
            None => title_by(title_author),
        };
        let then = if asked {
            "Your person has been asked.".to_string()
        } else {
            format!(
                "To ask your person, call ask with {{\"approval\": {{\"thread_id\": \"{thread_id}\"}}}}."
            )
        };
        format!(
            "[clax] {n} comments from {author} on \"{title}\" (thread {thread_id}) are withheld until your person approves them. {then}"
        )
    }

    /// A delivered comment's anchor line when its anchor is withheld.
    pub fn anchored_on(page_author: &str) -> String {
        format!("Anchored on: (withheld: page by {page_author})")
    }

    /// A delivered comment's clip line when its clip is withheld.
    pub const CLIP: &str = "Clip: withheld";

    /// A delivered comment's batch note when it is withheld.
    pub const NOTE: &str = "(note withheld)";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Caller, Level};
    use crate::publish::{PublishRequest, validate};
    use crate::store::approvals::{Scope, Via};
    use crate::store::docs::Pin;
    use crate::store::people::tests::{approved, store};
    use crate::store::test_util::{DAEMON, anchor, artifact_with_caps};
    use crate::store::threads::NewThread;
    use crate::{ArtifactId, RegisterSession};
    use rand::{Rng as _, SeedableRng as _, rngs::StdRng};
    use serde_json::{Value, json};

    /// The owner, Sam and Mia (connected people), and Lan (a LAN viewer
    /// who is not connected), on a store at migration 25.
    struct World {
        _dir: tempfile::TempDir,
        st: Store,
        owner: String,
        sam: String,
        mia: String,
        lan: String,
        sam_grant: String,
    }

    fn two_people() -> World {
        let (dir, _clock, st) = store();
        let owner = st.owner_viewer(true).unwrap().public_id;
        let (sam_grant, _, _) = approved(&st, "Sam", [1; 32]);
        let sam = st.viewer_of_grant(&sam_grant).unwrap().public_id;
        let (mia_grant, _, _) = approved(&st, "Mia", [2; 32]);
        let mia = st.viewer_of_grant(&mia_grant).unwrap().public_id;
        let lan = st
            .upsert_viewer("01ARZ3NDEKTSV4RRFFQ69G5FAV", Some("Lan"))
            .unwrap()
            .public_id;
        World {
            _dir: dir,
            st,
            owner,
            sam,
            mia,
            lan,
            sam_grant,
        }
    }

    fn files(fs: &[(&str, &str)]) -> Value {
        let mut m = serde_json::Map::new();
        for (p, c) in fs {
            m.insert((*p).to_string(), json!({"content": c, "encoding": "utf8"}));
        }
        Value::Object(m)
    }

    fn request(v: Value, person: Option<&str>, by_page: bool) -> crate::publish::ValidatedPublish {
        let req: PublishRequest = serde_json::from_value(v).unwrap();
        let mut p = validate(req).unwrap();
        p.person = person.map(str::to_string);
        p.by_page = by_page;
        p
    }

    impl World {
        fn reader(&self, r: &Reader) -> Visibility {
            Visibility::for_reader(&self.st, r, self.st.visibility_generation()).unwrap()
        }

        /// An artifact `person` publishes (`None`: the owner) with `fs`.
        fn publish_as(&self, person: Option<&str>, fs: &[(&str, &str)]) -> ArtifactId {
            let p = request(json!({"title": "T", "files": files(fs)}), person, false);
            let (a, _) = self.st.create_artifact(DAEMON, p, None).unwrap();
            ArtifactId::parse(&a.id).unwrap()
        }

        /// The next version of `aid`, published by `person` (or by a page
        /// of it, `by_page`), writing `fs` and carrying the rest.
        fn publish_next(
            &self,
            aid: &ArtifactId,
            person: Option<&str>,
            by_page: bool,
            fs: &[(&str, &str)],
        ) {
            let cur = self.st.get_artifact(aid).unwrap().unwrap().current_version;
            let p = request(
                json!({"if_version": cur, "files": files(fs)}),
                person,
                by_page,
            );
            self.st.publish_version(DAEMON, aid, p, None).unwrap();
        }

        fn version(&self, aid: &ArtifactId, n: u32) -> Version {
            self.st.get_version(aid, n).unwrap().unwrap()
        }

        /// A thread on version 1 of `aid` by viewer `by` (its first
        /// comment), written by the page when `via_page`.
        fn thread_by(&self, aid: &ArtifactId, by: Option<&str>, via_page: bool) -> Thread {
            self.st
                .create_thread(
                    DAEMON,
                    aid,
                    NewThread {
                        version_n: 1,
                        anchor: crate::Anchor {
                            file: "index.html".into(),
                            ..anchor()
                        },
                        author_name: "x".into(),
                        author_public_id: by.map(str::to_string),
                        body: "b".into(),
                        clip: None,
                        via_page,
                    },
                )
                .unwrap()
        }

        fn db_artifact(&self) -> ArtifactId {
            artifact_with_caps(&self.st, json!({"db": {}}))
        }

        fn write_doc(
            &self,
            aid: &ArtifactId,
            person: Author,
            via_page: bool,
            op: &str,
            path: &str,
            data: Value,
        ) {
            let caller = Caller {
                level: Level::Owner,
                viewer: None,
                person,
                via_page,
            };
            let pin = Pin {
                if_version: None,
                lww: true,
            };
            match op {
                "set" => self
                    .st
                    .doc_set(DAEMON, aid, path, data, pin, &caller)
                    .map(|_| ()),
                "update" => self
                    .st
                    .doc_update(DAEMON, aid, path, data, pin, &caller)
                    .map(|_| ()),
                "delete" => self
                    .st
                    .doc_delete(DAEMON, aid, path, pin, &caller)
                    .map(|_| ()),
                _ => unreachable!(),
            }
            .unwrap();
        }
    }

    fn p(s: &str) -> Author {
        Author::Person(s.to_string())
    }

    #[test]
    fn own_person_is_always_visible_and_others_need_approval() {
        let w = two_people();
        let sam = w.reader(&Reader::Person(w.sam.clone()));
        assert!(sam.sees(&[p(&w.sam)], "a1", Some("t1")));
        assert!(!sam.sees(&[p(&w.mia)], "a1", Some("t1")));
        assert!(!sam.sees(&[Author::Owner], "a1", None));
        assert!(
            !sam.sees(&[p(&w.owner)], "a1", None),
            "the owner's public ID is the owner"
        );
        assert!(
            !sam.sees(&[Author::Unconnected], "a1", None),
            "*unconnected is the owner's approval only"
        );
        assert!(!sam.sees(&[p(&w.lan)], "a1", None));
        assert!(sam.sees(&[Author::System], "a1", None));
        let owner = w.reader(&Reader::Owner);
        assert!(!owner.is_all(), "people are present");
        assert!(owner.sees(&[Author::Unconnected], "a1", None));
        assert!(
            owner.sees(&[p(&w.lan)], "a1", None),
            "a LAN viewer is *unconnected"
        );
        assert!(owner.sees(&[Author::Owner, p(&w.owner)], "a1", None));
        assert!(!owner.sees(&[p(&w.sam)], "a1", None));
    }

    #[test]
    fn scopes_thread_artifact_everywhere_and_revocation() {
        let w = two_people();
        let aid = w.publish_as(Some(&w.mia), &[("index.html", "<p>m</p>")]);
        let t1 = w.thread_by(&aid, Some(&w.mia), false).id;
        let t2 = w.thread_by(&aid, Some(&w.mia), false).id;
        let other = w.publish_as(Some(&w.mia), &[("index.html", "<p>o</p>")]);
        let (a, o) = (aid.as_str(), other.as_str());
        let sam = Reader::Person(w.sam.clone());
        let mia = [p(&w.mia)];
        let approve = |scope| {
            w.st.approve(DAEMON, Some(&w.sam), &w.mia, scope, Via::Shell)
                .unwrap()
                .id
        };
        let th = approve(Scope::Thread(t1.clone()));
        let v = w.reader(&sam);
        assert!(v.sees(&mia, a, Some(&t1)));
        // A later reply in the thread is covered (R1): the scope is the thread.
        let reply =
            w.st.add_comment(
                DAEMON,
                &t1,
                crate::NewComment {
                    author_kind: crate::store::threads::AUTHOR_VIEWER,
                    author_name: "Mia".into(),
                    author_public_id: Some(w.mia.clone()),
                    via_session_id: None,
                    body: "later".into(),
                    via_page: false,
                },
            )
            .unwrap();
        assert!(v.sees(&authors_of_comment(&reply), a, Some(&t1)));
        assert!(!v.sees(&mia, a, Some(&t2)), "another thread");
        assert!(!v.sees(&mia, a, None), "outside any thread");
        let art = approve(Scope::Artifact(a.to_string()));
        let v = w.reader(&sam);
        assert!(v.sees(&mia, a, Some(&t2)) && v.sees(&mia, a, None));
        assert!(!v.sees(&mia, o, None), "another artifact");
        let all = approve(Scope::Everywhere);
        let v = w.reader(&sam);
        assert!(v.sees(&mia, o, None) && v.sees(&mia, "anywhere", Some("t")));
        // Approvals are the approver's own: Mia's agents gain nothing, the
        // owner's neither.
        assert!(!w.reader(&Reader::Owner).sees(&mia, a, Some(&t1)));
        assert!(
            !w.reader(&Reader::Person(w.mia.clone()))
                .sees(&[p(&w.sam)], a, None)
        );
        for id in [all, art, th] {
            w.st.revoke_approval(DAEMON, &id, Some(&w.sam)).unwrap();
        }
        let v = w.reader(&sam);
        assert!(!v.sees(&mia, a, Some(&t1)) && !v.sees(&mia, o, None));
    }

    #[test]
    fn every_author_of_a_field_must_be_visible() {
        let w = two_people();
        w.st.approve(
            DAEMON,
            Some(&w.sam),
            &w.owner,
            Scope::Everywhere,
            Via::Connect,
        )
        .unwrap();
        let v = w.reader(&Reader::Person(w.sam.clone()));
        assert!(v.sees(&[Author::Owner, p(&w.sam)], "a", None));
        assert!(!v.sees(&[Author::Owner, p(&w.mia)], "a", None));
        assert!(!v.sees(&[p(&w.sam), Author::Unconnected], "a", None));
        assert_eq!(
            v.withheld_authors(
                &[p(&w.owner), p(&w.mia), Author::Unconnected, p(&w.mia)],
                "a",
                None
            ),
            vec![p(&w.mia), Author::Unconnected],
            "normalised, once each, in order"
        );
        assert!(v.sees(&[], "a", None), "no author withholds nothing");
    }

    #[test]
    fn page_written_content_counts_its_publisher_fixed_at_write() {
        let w = two_people();
        // v1 by Sam's agent; a page comment by the owner's browser on it.
        let aid = w.publish_as(Some(&w.sam), &[("index.html", "<p>sam</p>")]);
        let t = w.thread_by(&aid, Some(&w.owner), true);
        let reply =
            w.st.add_comment(
                DAEMON,
                &t.id,
                crate::NewComment {
                    author_kind: crate::store::threads::AUTHOR_VIEWER,
                    author_name: "Lan".into(),
                    author_public_id: Some(w.lan.clone()),
                    via_session_id: None,
                    body: "via the page".into(),
                    via_page: true,
                },
            )
            .unwrap();
        // The owner publishes v2 afterwards: the comments keep Sam.
        w.publish_next(&aid, None, false, &[("index.html", "<p>owner</p>")]);
        let thread = w.st.get_thread(&t.id).unwrap().unwrap();
        assert_eq!(
            normalise(&authors_of_comment(&thread.comments[0]), Some(&w.owner)),
            vec![Author::Owner, p(&w.sam)],
            "fixed at write, not the current publisher"
        );
        assert_eq!(
            authors_of_comment(&thread.comments[1]),
            vec![p(&w.lan), p(&w.sam)]
        );
        assert_eq!(
            authors_of_comment(&reply),
            vec![p(&w.lan), p(&w.sam)],
            "as returned"
        );
        // A page comment on the owner's own page counts the owner.
        let mine = w.publish_as(None, &[("index.html", "<p>mine</p>")]);
        let t = w.thread_by(&mine, Some(&w.lan), true);
        assert_eq!(
            authors_of_comment(&t.comments[0]),
            vec![p(&w.lan), Author::Owner]
        );
        // Not via the page: the writer alone.
        let t = w.thread_by(&aid, None, false);
        assert_eq!(
            authors_of_comment(&t.comments[0]),
            vec![Author::Unconnected]
        );
    }

    #[test]
    fn a_page_published_version_counts_the_page_s_publisher() {
        let w = two_people();
        let aid = w.publish_as(
            Some(&w.mia),
            &[("index.html", "<p>m</p>"), ("a.css", "a{}")],
        );
        // The owner's browser runs Mia's page, which publishes itself.
        w.publish_next(&aid, None, true, &[("index.html", "<p>page</p>")]);
        let v2 = w.version(&aid, 2);
        assert_eq!(authors_of_version(&v2), vec![Author::Owner, p(&w.mia)]);
        assert_eq!(
            authors_of_file(&v2, "index.html"),
            vec![Author::Owner, p(&w.mia)]
        );
        assert_eq!(authors_of_file(&v2, "a.css"), vec![p(&w.mia)], "carried");
        let a = w.st.get_artifact(&aid).unwrap().unwrap();
        assert_eq!(authors_of_artifact_text(&a), vec![p(&w.mia), Author::Owner]);
    }

    #[test]
    fn touching_a_field_never_takes_a_document() {
        let w = two_people();
        let a1 = w.db_artifact();
        w.write_doc(
            &a1,
            p(&w.mia),
            false,
            "set",
            "notes/1",
            json!({"text": "MIA", "n": 1}),
        );
        w.write_doc(&a1, p(&w.sam), false, "update", "notes/1", json!({"n": 2}));
        assert_eq!(
            w.st.doc_authors(&a1, "notes/1").unwrap(),
            vec![p(&w.mia), p(&w.sam)]
        );
        w.write_doc(
            &a1,
            p(&w.sam),
            false,
            "set",
            "notes/1",
            json!({"text": "mine"}),
        );
        assert_eq!(
            w.st.doc_authors(&a1, "notes/1").unwrap(),
            vec![p(&w.mia), p(&w.sam)],
            "a full set adds, never replaces"
        );
        // Deleting it and writing anew does (§21.7).
        w.write_doc(&a1, p(&w.sam), false, "delete", "notes/1", Value::Null);
        w.write_doc(
            &a1,
            p(&w.sam),
            false,
            "set",
            "notes/1",
            json!({"text": "new"}),
        );
        assert_eq!(w.st.doc_authors(&a1, "notes/1").unwrap(), vec![p(&w.sam)]);
    }

    #[test]
    fn an_owners_document_stays_the_owners_as_others_join() {
        let w = two_people();
        let a1 = w.db_artifact();
        w.write_doc(&a1, Author::Owner, false, "set", "notes/1", json!({"n": 1}));
        assert_eq!(
            w.st.doc_authors(&a1, "notes/1").unwrap(),
            vec![Author::Owner]
        );
        let rows = |st: &Store| {
            st.with_read(|c| {
                Ok(c.query_row("SELECT COUNT(*) FROM doc_authors", [], |r| {
                    r.get::<_, i64>(0)
                })?)
            })
            .unwrap()
        };
        assert_eq!(rows(&w.st), 0, "the owner's alone needs no rows");
        w.write_doc(&a1, p(&w.mia), false, "update", "notes/1", json!({"n": 2}));
        assert_eq!(
            w.st.doc_authors(&a1, "notes/1").unwrap(),
            vec![Author::Owner, p(&w.mia)]
        );
        // A page's write: the browser's viewer and the page's publisher.
        let page = w.st.create_artifact(
            DAEMON,
            request(json!({"title": "db", "capabilities": {"db": {}}, "files": files(&[("index.html", "<p/>")])}), Some(&w.sam), false),
            None,
        ).unwrap().0;
        let page = ArtifactId::parse(&page.id).unwrap();
        w.write_doc(&page, p(&w.lan), true, "set", "notes/2", json!({"n": 1}));
        assert_eq!(
            w.st.doc_authors(&page, "notes/2").unwrap(),
            vec![p(&w.lan), p(&w.sam)]
        );
        w.write_doc(
            &page,
            Author::Owner,
            true,
            "set",
            "notes/3",
            json!({"n": 1}),
        );
        assert_eq!(
            w.st.doc_authors(&page, "notes/3").unwrap(),
            vec![Author::Owner, p(&w.sam)]
        );
        // Deleting the artifact drops its documents' authors.
        w.st.delete_artifact(DAEMON, &page).unwrap();
        let left: i64 =
            w.st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM doc_authors WHERE artifact_id = ?1",
                    [page.as_str()],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn carrying_a_file_forward_keeps_its_author() {
        let w = two_people();
        let aid = w.publish_as(
            Some(&w.mia),
            &[("index.html", "<p>MIA</p>"), ("app.css", "a{}")],
        );
        // The owner writes app.css and index.html, carrying nothing but...
        w.publish_next(
            &aid,
            None,
            false,
            &[("index.html", "<p>MIA</p>"), ("app.css", "b{}")],
        );
        // ...here index.html is always written; a third file is carried.
        let aid2 = w.publish_as(
            Some(&w.mia),
            &[
                ("index.html", "<p>MIA</p>"),
                ("app.css", "a{}"),
                ("data.json", "{}"),
            ],
        );
        w.publish_next(
            &aid2,
            None,
            false,
            &[("index.html", "<p>o</p>"), ("app.css", "b{}")],
        );
        let v2 = w.version(&aid2, 2);
        assert_eq!(
            authors_of_file(&v2, "data.json"),
            vec![p(&w.mia)],
            "carried forward"
        );
        assert_eq!(authors_of_file(&v2, "app.css"), vec![Author::Owner]);
        assert_eq!(authors_of_file(&v2, "index.html"), vec![Author::Owner]);
        // Carried again by Sam's agent, it is still Mia's.
        w.publish_next(&aid2, Some(&w.sam), false, &[("index.html", "<p>s</p>")]);
        let v3 = w.version(&aid2, 3);
        assert_eq!(authors_of_file(&v3, "data.json"), vec![p(&w.mia)]);
        assert_eq!(authors_of_file(&v3, "app.css"), vec![Author::Owner]);
        assert_eq!(authors_of_file(&v3, "index.html"), vec![p(&w.sam)]);
        assert_eq!(authors_of_version(&v3), vec![p(&w.sam)]);
        // An owner's version alone stores no file authors.
        let mine = w.publish_as(None, &[("index.html", "<p>o</p>"), ("x.css", "")]);
        assert_eq!(w.version(&mine, 1).authors.files, None);
        assert_eq!(
            authors_of_file(&w.version(&mine, 1), "x.css"),
            vec![Author::Owner]
        );
    }

    #[test]
    fn anchors_clips_and_the_rest_take_their_authors_from_their_rows() {
        let w = two_people();
        let aid = w.publish_as(Some(&w.mia), &[("index.html", "<p>m</p>")]);
        let t = w.thread_by(&aid, Some(&w.lan), false);
        let v1 = w.version(&aid, 1);
        assert_eq!(authors_of_anchor(&t, &v1), vec![p(&w.mia)]);
        assert_eq!(authors_of_clip(&t, &v1), vec![p(&w.lan), p(&w.mia)]);
        assert_eq!(
            authors_of_resolved_by(&format!("viewer:{}", w.sam)),
            vec![p(&w.sam)]
        );
        assert_eq!(
            authors_of_resolved_by("viewer:anonymous"),
            vec![Author::Unconnected]
        );
        assert_eq!(
            authors_of_resolved_by("agent:claude"),
            vec![Author::Unconnected]
        );
        assert_eq!(authors_of_batch_note(Some(&w.sam)), vec![p(&w.sam)]);
        assert_eq!(authors_of_batch_note(None), vec![Author::Unconnected]);
        assert_eq!(authors_of_working(Some(&w.sam)), vec![p(&w.sam)]);
        assert_eq!(authors_of_working(None), vec![Author::Owner]);
        assert_eq!(authors_of_doc(&[]), vec![Author::Owner]);
        assert_eq!(
            authors_of_doc(&["*owner".into(), "*unconnected".into()]),
            vec![Author::Owner, Author::Unconnected]
        );
        let asset =
            w.st.add_asset_by(DAEMON, &aid, "text/plain", b"x", Some(&w.mia))
                .unwrap();
        assert_eq!(authors_of_asset(&asset), vec![p(&w.mia)]);
        let listed = w.st.list_assets(&aid).unwrap();
        assert_eq!(authors_of_asset(&listed[0]), vec![p(&w.mia)]);
        let a = w.st.get_artifact(&aid).unwrap().unwrap();
        assert_eq!(authors_of_artifact_text(&a), vec![p(&w.mia)]);
        // An agent comment is its session's person's.
        let s =
            w.st.register_session(
                DAEMON,
                RegisterSession {
                    harness: "claude".into(),
                    cwd: "/w".into(),
                    grant_id: Some(w.sam_grant.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        let c =
            w.st.add_comment(
                DAEMON,
                &t.id,
                crate::NewComment {
                    author_kind: crate::store::threads::AUTHOR_AGENT,
                    author_name: "claude".into(),
                    author_public_id: None,
                    via_session_id: Some(s.id),
                    body: "on it".into(),
                    via_page: false,
                },
            )
            .unwrap();
        assert_eq!(authors_of_comment(&c), vec![p(&w.sam)]);
        let loaded = w.st.get_thread(&t.id).unwrap().unwrap();
        assert_eq!(authors_of_comment(&loaded.comments[1]), vec![p(&w.sam)]);
        // Never served.
        let json = serde_json::to_value(&loaded).unwrap();
        assert!(
            !json.to_string().contains("page_person") && !json.to_string().contains("agent_person")
        );
        assert!(serde_json::to_value(&a).unwrap().get("authors").is_none());
        assert!(serde_json::to_value(&v1).unwrap().get("authors").is_none());
        assert!(
            serde_json::to_value(&listed[0])
                .unwrap()
                .get("person")
                .is_none()
        );
    }

    #[test]
    fn a_fold_moves_every_authorship_column() {
        let w = two_people();
        // A LAN viewer minted on this machine writes a document, a page
        // comment on a page, and gets an approval from Sam.
        let cookie = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
        let me = w.st.mint_viewer(cookie, true).unwrap().public_id;
        let a1 = w.db_artifact();
        w.write_doc(&a1, p(&me), false, "set", "notes/1", json!({"n": 1}));
        w.write_doc(&a1, p(&w.mia), false, "update", "notes/1", json!({"n": 2}));
        let aid = w.publish_as(Some(&w.mia), &[("index.html", "<p>m</p>")]);
        let t = w.thread_by(&aid, Some(&me), true);
        w.st.approve(DAEMON, Some(&w.sam), &me, Scope::Everywhere, Via::Shell)
            .unwrap();
        w.st.approve(
            DAEMON,
            Some(&w.sam),
            &w.owner,
            Scope::Everywhere,
            Via::Shell,
        )
        .unwrap();
        // Its page-written comment counted Mia; mark the comment's
        // publisher as the viewer to see it fold too.
        w.st.with_write(|c| {
            c.execute(
                "UPDATE comments SET page_person = ?1 WHERE thread_id = ?2",
                rusqlite::params![me, t.id],
            )?;
            c.execute(
                "UPDATE versions SET file_authors_json = ?1 WHERE artifact_id = ?2",
                rusqlite::params![json!({"index.html": [me, w.mia]}).to_string(), aid.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
        let g = w.st.visibility_generation();
        w.st.claim_for_owner(crate::audit::Via::Shell, cookie)
            .unwrap();
        assert_eq!(w.st.visibility_generation(), g + 1);
        assert_eq!(
            w.st.doc_authors(&a1, "notes/1").unwrap(),
            vec![Author::Owner, p(&w.mia)]
        );
        let thread = w.st.get_thread(&t.id).unwrap().unwrap();
        assert_eq!(thread.comments[0].page_person.as_deref(), Some(OWNER));
        assert_eq!(
            authors_of_file(&w.version(&aid, 1), "index.html"),
            vec![Author::Owner, p(&w.mia)]
        );
        // Sam's two approvals now both name the owner: one stays live.
        let live = w.st.live_approvals(Some(&w.sam)).unwrap();
        assert_eq!(live.len(), 1, "{live:?}");
        assert_eq!(live[0].author, w.owner);
    }

    #[test]
    fn folds_and_person_revocation_bump_the_generation() {
        let w = two_people();
        let cookie = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
        w.st.mint_viewer(cookie, true).unwrap();
        let g = w.st.visibility_generation();
        w.st.claim_for_owner(crate::audit::Via::Shell, cookie)
            .unwrap();
        assert_eq!(w.st.visibility_generation(), g + 1, "a fold");
        w.st.revoke_person(DAEMON, &w.sam).unwrap();
        // Sam's grant ends and Sam is revoked, in one write: one bump.
        assert_eq!(w.st.visibility_generation(), g + 2, "a person's revocation");
    }

    #[test]
    fn a_stale_visibility_refreshes_on_a_new_generation() {
        let w = two_people();
        let mut v = Visibility::for_reader(&w.st, &Reader::Person(w.sam.clone()), 1).unwrap();
        let id =
            w.st.approve(DAEMON, Some(&w.sam), &w.mia, Scope::Everywhere, Via::Shell)
                .unwrap()
                .id;
        v.refresh_if_stale(&w.st, 1).unwrap();
        assert!(
            !v.sees(&[p(&w.mia)], "a1", None),
            "the same generation keeps the view"
        );
        v.refresh_if_stale(&w.st, 2).unwrap();
        assert!(v.sees(&[p(&w.mia)], "a1", None));
        assert_eq!(v.generation(), 2);
        w.st.revoke_approval(DAEMON, &id, Some(&w.sam)).unwrap();
        v.refresh_if_stale(&w.st, 3).unwrap();
        assert!(!v.sees(&[p(&w.mia)], "a1", None));
        let mut all = Visibility::all();
        all.refresh_if_stale(&w.st, 9).unwrap();
        assert!(all.is_all() && all.reader().is_none());
    }

    #[test]
    fn single_owner_install_takes_the_fast_path() {
        let (_d, _c, st) = store();
        let g = st.visibility_generation();
        let mut v = Visibility::for_reader(&st, &Reader::Owner, g).unwrap();
        assert!(v.is_all());
        assert!(v.sees(
            &[
                Author::Unconnected,
                Author::Owner,
                p("u_0000000000000000000000")
            ],
            "a",
            None
        ));
        // A person arrives: the next refresh filters.
        st.create_invite(
            DAEMON,
            crate::store::people::InvitePerson::New("Sam".into()),
            None,
        )
        .unwrap();
        v.refresh_if_stale(&st, st.visibility_generation()).unwrap();
        assert!(!v.is_all());
        let sam = st.list_invites().unwrap()[0].person.clone();
        assert!(!v.sees(&[p(&sam)], "a", None));
        assert!(v.sees(&[Author::Unconnected], "a", None));
    }

    #[test]
    fn for_reader_reads_approvals_with_one_indexed_query() {
        let w = two_people();
        // Many approvers' approvals, so a scan would cost; planned with
        // and without statistics.
        w.st.with_write(|c| {
            c.execute_batch("BEGIN")?;
            for i in 0..400 {
                c.execute(
                    "INSERT INTO approvals (id, approver, author, scope, artifact_id, created_at, created_via)
                     VALUES (?1, ?2, ?3, 'artifact', ?4, 'x', 'shell')",
                    rusqlite::params![format!("ap{i}"), format!("u_{:022}", i % 50), format!("u_{:022}", i % 7), format!("a{i}")],
                )?;
            }
            c.execute_batch("COMMIT")?;
            Ok(())
        })
        .unwrap();
        let plan = |c: &rusqlite::Connection| -> Vec<String> {
            let mut stmt = c
                .prepare(&format!("EXPLAIN QUERY PLAN {APPROVALS_OF}"))
                .unwrap();
            stmt.query_map([w.sam.as_str()], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        w.st.with_write(|c| {
            for stats in [
                "DELETE FROM sqlite_stat1; ANALYZE sqlite_schema;",
                "ANALYZE",
            ] {
                c.execute_batch(stats)?;
                let plan = plan(c);
                assert_eq!(plan.len(), 1, "{stats}: {plan:?}");
                assert!(
                    plan[0].contains("USING INDEX approvals_by_approver"),
                    "{stats}: {plan:?}"
                );
            }
            Ok(())
        })
        .unwrap();
        // And the reader's view reads exactly its approvals.
        let n: i64 =
            w.st.with_read(|c| {
                let mut stmt = c.prepare(APPROVALS_OF)?;
                Ok(stmt
                    .query_map([format!("u_{:022}", 3)], |_| Ok(()))?
                    .count() as i64)
            })
            .unwrap();
        assert_eq!(n, 8);
    }

    /// The expected answer, written from §8.1's three rules, for the
    /// matrix: `approvals` are (approver key, author key, scope).
    fn oracle(
        reader: &str,
        author: &str,
        people: &[&str],
        approvals: &[(String, String, String)],
        artifact: &str,
        thread: Option<&str>,
    ) -> bool {
        if author == "system" || author == reader {
            return true;
        }
        let covers = |key: &str| {
            approvals.iter().any(|(by, who, scope)| {
                by == reader
                    && who == key
                    && (scope == "everywhere"
                        || *scope == format!("artifact:{artifact}")
                        || thread.is_some_and(|t| *scope == format!("thread:{t}")))
            })
        };
        let unconnected_class = author != "owner" && !people.contains(&author);
        covers(author) || (unconnected_class && covers("*unconnected"))
    }

    /// Every author × reader × approval scope × `*unconnected` switch.
    #[test]
    fn the_filter_matches_the_rules_for_every_combination() {
        let w = two_people();
        let aid = w.publish_as(None, &[("index.html", "<p/>")]);
        let tid = w.thread_by(&aid, None, false).id;
        let other = w.publish_as(None, &[("index.html", "<p/>")]);
        let tid2 = w.thread_by(&other, None, false).id;
        // Keys: the owner, Sam, Mia, Lan (not connected), anonymous, Clax.
        let key = |a: &Author| match a {
            Author::Owner => "owner".to_string(),
            Author::Person(x) if *x == w.owner => "owner".into(),
            Author::Person(x) => x.clone(),
            Author::Unconnected => "*unconnected".into(),
            Author::System => "system".into(),
        };
        let authors = [
            Author::Owner,
            p(&w.owner),
            p(&w.sam),
            p(&w.mia),
            p(&w.lan),
            Author::Unconnected,
            Author::System,
        ];
        let readers = [
            (Reader::Owner, None, "owner".to_string()),
            (
                Reader::Person(w.sam.clone()),
                Some(w.sam.clone()),
                w.sam.clone(),
            ),
            (
                Reader::Person(w.mia.clone()),
                Some(w.mia.clone()),
                w.mia.clone(),
            ),
        ];
        let scopes = [
            None,
            Some(Scope::Thread(tid.clone())),
            Some(Scope::Thread(tid2.clone())),
            Some(Scope::Artifact(aid.to_string())),
            Some(Scope::Artifact(other.to_string())),
            Some(Scope::Everywhere),
        ];
        let people = [w.sam.as_str(), w.mia.as_str()];
        let switch = w.st.live_approvals(None).unwrap()[0].id.clone();
        let mut checked = 0;
        for switch_on in [true, false] {
            if !switch_on {
                w.st.revoke_approval(DAEMON, &switch, None).unwrap();
            }
            for (reader, approver, reader_key) in &readers {
                for author in &authors {
                    for scope in &scopes {
                        // At most one approval of `author` by the reader,
                        // plus the switch.
                        let mut made = Vec::new();
                        let mut table: Vec<(String, String, String)> = Vec::new();
                        if switch_on {
                            table.push((
                                "owner".into(),
                                "*unconnected".into(),
                                "everywhere".into(),
                            ));
                        }
                        let stored_author = match author {
                            Author::Owner => Some(w.owner.clone()),
                            Author::Person(x) => Some(x.clone()),
                            Author::Unconnected if approver.is_none() => {
                                Some(UNCONNECTED.to_string())
                            }
                            _ => None,
                        };
                        if let (Some(s), Some(a)) = (scope, &stored_author)
                            && let Ok(made_one) =
                                w.st.approve(DAEMON, approver.as_deref(), a, s.clone(), Via::Shell)
                        {
                            if made_one.id != switch {
                                made.push(made_one.id);
                            }
                            let scope_key = match s {
                                Scope::Everywhere => "everywhere".to_string(),
                                Scope::Artifact(x) => format!("artifact:{x}"),
                                Scope::Thread(x) => format!("thread:{x}"),
                            };
                            let akey = if *a == w.owner {
                                "owner".to_string()
                            } else {
                                a.clone()
                            };
                            table.push((reader_key.clone(), akey, scope_key));
                        }
                        let v = w.reader(reader);
                        for (art, thread) in [
                            (aid.as_str(), Some(tid.as_str())),
                            (aid.as_str(), None),
                            (other.as_str(), Some(tid2.as_str())),
                            (other.as_str(), None),
                        ] {
                            let want =
                                oracle(reader_key, &key(author), &people, &table, art, thread);
                            assert_eq!(
                                v.sees(std::slice::from_ref(author), art, thread),
                                want,
                                "reader {reader:?}, author {author:?}, scope {scope:?}, switch {switch_on}, at {art} {thread:?}"
                            );
                            assert_eq!(
                                v.withheld_authors(std::slice::from_ref(author), art, thread)
                                    .is_empty(),
                                want
                            );
                            checked += 1;
                        }
                        for id in made {
                            w.st.revoke_approval(DAEMON, &id, approver.as_deref())
                                .unwrap();
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 2 * 3 * 7 * 6 * 4);
    }

    /// Seeded random worlds: random approvals by the owner, Sam and Mia,
    /// random author lists, and the properties every view must keep.
    #[test]
    fn seeded_random_views_keep_the_invariants() {
        let w = two_people();
        let arts: Vec<ArtifactId> = (0..3)
            .map(|_| w.publish_as(None, &[("index.html", "<p/>")]))
            .collect();
        let threads: Vec<(usize, String)> = (0..6)
            .map(|i| (i % 3, w.thread_by(&arts[i % 3], None, false).id))
            .collect();
        let pool = [
            Author::Owner,
            p(&w.owner),
            p(&w.sam),
            p(&w.mia),
            p(&w.lan),
            Author::Unconnected,
            Author::System,
        ];
        let approvers = [None, Some(w.sam.clone()), Some(w.mia.clone())];
        let mut rng = StdRng::seed_from_u64(0x0815_c1a5);
        for round in 0..40 {
            // Random approvals.
            let mut made = Vec::new();
            for _ in 0..rng.random_range(0..8) {
                let by = approvers[rng.random_range(0..3)].clone();
                let author = match rng.random_range(0..5) {
                    0 => w.owner.clone(),
                    1 => w.sam.clone(),
                    2 => w.mia.clone(),
                    3 => w.lan.clone(),
                    _ => UNCONNECTED.to_string(),
                };
                let scope = match rng.random_range(0..3) {
                    0 => Scope::Everywhere,
                    1 => Scope::Artifact(arts[rng.random_range(0..3)].to_string()),
                    _ => Scope::Thread(threads[rng.random_range(0..6)].1.clone()),
                };
                if let Ok(a) =
                    w.st.approve(DAEMON, by.as_deref(), &author, scope, Via::Shell)
                {
                    made.push((by, a.id));
                }
            }
            let switch_on = rng.random_bool(0.5);
            let switch =
                w.st.live_approvals(None)
                    .unwrap()
                    .into_iter()
                    .find(|a| a.author == UNCONNECTED);
            match (switch_on, &switch) {
                (false, Some(a)) => {
                    w.st.revoke_approval(DAEMON, &a.id, None).unwrap();
                }
                (true, None) => {
                    w.st.approve(DAEMON, None, UNCONNECTED, Scope::Everywhere, Via::Shell)
                        .unwrap();
                }
                _ => {}
            }
            for (approver, reader) in [
                (None, Reader::Owner),
                (Some(w.sam.clone()), Reader::Person(w.sam.clone())),
                (Some(w.mia.clone()), Reader::Person(w.mia.clone())),
            ] {
                let v = w.reader(&reader);
                let live = w.st.live_approvals(approver.as_deref()).unwrap();
                for _ in 0..30 {
                    let n = rng.random_range(1..4);
                    let authors: Vec<Author> = (0..n)
                        .map(|_| pool[rng.random_range(0..pool.len())].clone())
                        .collect();
                    let (ai, tid) = &threads[rng.random_range(0..6)];
                    let art = arts[*ai].as_str();
                    let thread = rng.random_bool(0.5).then_some(tid.as_str());
                    let seen = v.sees(&authors, art, thread);
                    let ctx =
                        format!("round {round}, {reader:?} on {authors:?} at {art} {thread:?}");
                    // A field is seen exactly when each author alone is.
                    assert_eq!(
                        seen,
                        authors
                            .iter()
                            .all(|a| v.sees(std::slice::from_ref(a), art, thread)),
                        "{ctx}"
                    );
                    assert_eq!(
                        seen,
                        v.withheld_authors(&authors, art, thread).is_empty(),
                        "{ctx}"
                    );
                    let covering = |key: &str| {
                        live.iter().any(|a| {
                            a.author == key
                                && match a.scope {
                                    crate::store::approvals::ScopeKind::Everywhere => true,
                                    crate::store::approvals::ScopeKind::Artifact => {
                                        a.artifact_id.as_deref() == Some(art)
                                    }
                                    crate::store::approvals::ScopeKind::Thread => {
                                        thread.is_some() && a.thread_id.as_deref() == thread
                                    }
                                }
                        })
                    };
                    // Property 1: a person's reader never sees another
                    // person's content that no approval of theirs covers.
                    if let Reader::Person(me) = &reader {
                        for a in &authors {
                            let other = match a {
                                Author::Person(x)
                                    if x != me && x != &w.owner && (x == &w.sam || x == &w.mia) =>
                                {
                                    Some(x.as_str())
                                }
                                Author::Owner => Some(w.owner.as_str()),
                                Author::Person(x) if x == &w.owner => Some(w.owner.as_str()),
                                _ => None,
                            };
                            if let Some(o) = other
                                && v.sees(std::slice::from_ref(a), art, thread)
                            {
                                assert!(covering(o), "{ctx}: {o} seen without an approval");
                            }
                            if matches!(a, Author::Unconnected)
                                || matches!(a, Author::Person(x) if *x == w.lan)
                            {
                                assert!(
                                    !v.sees(std::slice::from_ref(a), art, thread)
                                        || covering(&w.lan),
                                    "{ctx}: a person never holds *unconnected"
                                );
                            }
                        }
                    }
                    // Property 2: the owner's own content always reaches
                    // the owner's agents, and every reader its own (M12);
                    // Clax's notes reach everyone.
                    let own: Vec<Author> = match &reader {
                        Reader::Owner => vec![Author::Owner, p(&w.owner), Author::System],
                        Reader::Person(me) => vec![p(me), Author::System],
                    };
                    assert!(v.sees(&own, art, thread), "{ctx}");
                    // Never reaching beyond the rules.
                    let rk = match &reader {
                        Reader::Owner => "owner".to_string(),
                        Reader::Person(x) => x.clone(),
                    };
                    let table: Vec<(String, String, String)> = live
                        .iter()
                        .map(|a| {
                            let who = if a.author == w.owner {
                                "owner".to_string()
                            } else {
                                a.author.clone()
                            };
                            let scope = match a.scope {
                                crate::store::approvals::ScopeKind::Everywhere => {
                                    "everywhere".to_string()
                                }
                                crate::store::approvals::ScopeKind::Artifact => {
                                    format!("artifact:{}", a.artifact_id.clone().unwrap())
                                }
                                crate::store::approvals::ScopeKind::Thread => {
                                    format!("thread:{}", a.thread_id.clone().unwrap())
                                }
                            };
                            (rk.clone(), who, scope)
                        })
                        .collect();
                    let keyed: Vec<String> = authors
                        .iter()
                        .map(|a| match a {
                            Author::Owner => "owner".to_string(),
                            Author::Person(x) if *x == w.owner => "owner".into(),
                            Author::Person(x) => x.clone(),
                            Author::Unconnected => "*unconnected".into(),
                            Author::System => "system".into(),
                        })
                        .collect();
                    let people = [w.sam.as_str(), w.mia.as_str()];
                    let want = keyed
                        .iter()
                        .all(|k| oracle(&rk, k, &people, &table, art, thread));
                    assert_eq!(seen, want, "{ctx}");
                }
            }
            // Clax's own channels see everything, whatever the approvals.
            let all = Visibility::all();
            assert!(all.sees(&pool, arts[0].as_str(), None));
            for (by, id) in made {
                let _ = w.st.revoke_approval(DAEMON, &id, by.as_deref());
            }
        }
    }

    #[test]
    fn placeholders_replace_fields_and_keep_the_object() {
        use placeholder::*;
        let authors = [WithheldAuthor {
            public_id: Some("u_1".into()),
            name: "Mia".into(),
        }];
        let mut c = json!({"id": "c1", "author_name": "Mia", "body": "secret", "created_at": "t"});
        comment(&mut c);
        assert_eq!(
            c,
            json!({"id": "c1", "author_name": "Mia", "body": null, "withheld": true, "created_at": "t"})
        );
        let mut t = json!({"id": "t1", "anchor": {"kind": "element", "quote": "q", "prefix": "p", "suffix": "s", "selector": "#x", "file": "index.html"},
                           "clip": "/c.png", "resolved_by": "viewer:u_1"});
        anchor(&mut t);
        clip(&mut t);
        resolved_by(&mut t);
        thread_summary(&mut t, 2, &authors, ApprovalState::Requested);
        assert_eq!(
            t,
            json!({"id": "t1", "anchor": {"kind": "element", "quote": null, "prefix": null, "suffix": null, "selector": null, "file": "index.html"},
                   "anchor_withheld": true, "clip_withheld": true, "resolved_by": null,
                   "withheld": {"count": 2, "authors": [{"public_id": "u_1", "name": "Mia"}], "approval": "requested"}})
        );
        let mut b = json!({"id": "b1", "note": "n"});
        batch_note(&mut b);
        assert_eq!(b["note"], Value::Null);
        let mut a = json!({"id": "a1", "title": "T", "description": "D", "icon": "I", "capabilities": {"db": {}}, "current_version": 2});
        artifact(&mut a, &authors);
        assert_eq!(
            a,
            json!({"id": "a1", "title": null, "description": null, "icon": null, "capabilities": null, "current_version": 2,
                   "withheld": {"authors": [{"public_id": "u_1", "name": "Mia"}]}})
        );
        let mut wr = json!({"key": "k", "message": "m"});
        working(&mut wr);
        assert_eq!(wr, json!({"key": "k", "message": null, "withheld": true}));
        let mut v = json!({"n": 2, "label": "L", "note": "N", "files": {"index.html": {}, "a.css": {}, "b.css": {}}});
        version_text(&mut v);
        version_files(&mut v, &["a.css", "b.css", "gone.css"]);
        assert_eq!(
            v,
            json!({"n": 2, "label": null, "note": null, "files": {"index.html": {}}, "files_withheld": 2})
        );
        assert_eq!(
            document("notes/1", 3, &authors),
            json!({"doc_id": "notes/1", "version": 3, "withheld": {"authors": [{"public_id": "u_1", "name": "Mia"}]}})
        );
        let mut s = json!({"id": "s1", "name": "notes.txt", "size": 3});
        asset(&mut s);
        assert_eq!(
            s,
            json!({"id": "s1", "name": null, "size": 3, "withheld": true})
        );
        assert_eq!(
            withheld_comments_line(2, "Mia", Some("Plan"), "Mia", "t1", false),
            "[clax] 2 comments from Mia on \"Plan\" (thread t1) are withheld until your person approves them. To ask your person, call ask with {\"approval\": {\"thread_id\": \"t1\"}}."
        );
        assert_eq!(
            withheld_comments_line(1, "Mia", None, "Sam", "t1", true),
            "[clax] 1 comments from Mia on \"an artifact by Sam\" (thread t1) are withheld until your person approves them. Your person has been asked."
        );
        assert_eq!(anchored_on("Mia"), "Anchored on: (withheld: page by Mia)");
        assert_eq!((CLIP, NOTE), ("Clip: withheld", "(note withheld)"));
    }
}

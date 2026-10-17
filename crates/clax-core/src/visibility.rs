//! Whose words may reach an agent (spec 2026-10-08-multi-agent-multi-person
//! §8): the reader an agent reads for, the authors of each field (§8.2),
//! the filter that answers "may an agent of reader R see a field whose
//! effective authors are A₁…Aₙ, at artifact Y (thread T)?" (§8.1), and the
//! placeholders that replace a withheld field (§8.3).
//!
//! The filter runs for agent channels only (§8.5); people's browsers and
//! the owner's CLI by hand see everything. The filter itself, which reads
//! the reader's approvals, is the store's (`store::visibility`).

use crate::model::{Artifact, Asset, Comment, Thread, Version, VersionAuthors};

/// The author class of every viewer who is neither the owner nor a
/// connected person, and of anonymous callers (M14).
pub const UNCONNECTED: &str = "*unconnected";

/// The owner as an author, where a public ID is not used (`doc_authors`,
/// `comments.page_person`, `versions.file_authors_json`).
pub const OWNER: &str = "*owner";

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

/// The authors of a page's write, as recorded (`page_authors`, or a
/// document's rows): none recorded is the owner (pre-migration rows).
fn page_set(tokens: &[String]) -> Vec<Author> {
    if tokens.is_empty() {
        return vec![Author::Owner];
    }
    let mut out = Vec::new();
    for t in tokens {
        push(&mut out, Author::of_token(Some(t)));
    }
    out
}

/// A comment's body (§8.2): an agent comment's is its session's person;
/// a viewer comment's is its writer (anonymous, or from before public
/// IDs: [`Author::Unconnected`]), and for one a page wrote (`via_page`)
/// also every author of the page at write time (`page_authors`; none
/// recorded: the owner).
pub fn authors_of_comment(c: &Comment) -> Vec<Author> {
    if c.author_kind == crate::model::AUTHOR_AGENT {
        return vec![Author::of_token(c.agent_person.as_deref())];
    }
    let mut out = vec![Author::of_viewer(c.author_public_id.as_deref())];
    if c.via_page {
        for a in page_set(&c.page_authors) {
            push(&mut out, a);
        }
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
        _ => vec![Author::of_token(a.person.as_deref())],
    }
}

/// A thread's anchor (quote, prefix, suffix, selector, and the other
/// fields the anchored page gave it): the authors of the anchored file in
/// the thread's version, `version` (the version's publisher when it has no
/// such file). An anchor on the whole artifact is on no page and holds no
/// one's words: it has no authors.
pub fn authors_of_anchor(thread: &Thread, version: &Version) -> Vec<Author> {
    if thread.anchor.is_whole() {
        return Vec::new();
    }
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

/// A thread's `resolved_by` text: `viewer:<public ID>` is that viewer,
/// and `viewer:anonymous` [`Author::Unconnected`]; `agent:<harness>` is the
/// person of the resolving agent, `person` (`threads.resolved_by_person`:
/// `*owner` or a public ID), or [`Author::Unconnected`] for a resolve from
/// before it was recorded.
pub fn authors_of_resolved_by(resolved_by: &str, person: Option<&str>) -> Vec<Author> {
    if let Some(p) = resolved_by.strip_prefix("viewer:") {
        if crate::is_public_id(p) {
            return vec![Author::Person(p.to_string())];
        }
        return vec![Author::Unconnected];
    }
    match person {
        Some(t) if resolved_by.starts_with("agent:") => vec![Author::of_token(Some(t))],
        _ => vec![Author::Unconnected],
    }
}

/// A batch's note: its sender (`send_batches.sender`: `*owner`, a public
/// ID or `*unconnected`); a batch from before senders were recorded is
/// [`Author::Unconnected`], since a LAN viewer may have sent it.
pub fn authors_of_batch_note(sender: Option<&str>) -> Vec<Author> {
    match sender {
        Some(t) => vec![Author::of_token(Some(t))],
        None => vec![Author::Unconnected],
    }
}

/// A version's label and note: its publisher, and for a version a page
/// published, every author of the page that published it (which its
/// written files record, so: every author of its files). Its files have
/// authors of their own ([`authors_of_file`]).
pub fn authors_of_version(version: &Version) -> Vec<Author> {
    let a = &version.authors;
    let mut out = vec![Author::of_token(a.person.as_deref())];
    if a.by_page {
        for path in version.files.keys() {
            for f in file_authors(a, path) {
                push(&mut out, f);
            }
        }
    }
    out
}

/// One of an artifact's text fields (`title`, `description`, `icon`,
/// `capabilities`): the authors of the publish or metadata change that
/// last set it; one never set since its authors were recorded is its
/// creator's.
pub fn authors_of_artifact_field(a: &Artifact, field: &str) -> Vec<Author> {
    match a.authors.text.as_ref().and_then(|t| t.get(field)) {
        Some(tokens) if !tokens.is_empty() => {
            let mut out = Vec::new();
            for t in tokens {
                push(&mut out, Author::of_token(t.as_deref()));
            }
            out
        }
        _ => vec![Author::of_token(a.authors.creator.as_deref())],
    }
}

/// An artifact's title, description, icon and `capabilities` together
/// (they are withheld together, §8.3): every author of each.
pub fn authors_of_artifact_text(a: &Artifact) -> Vec<Author> {
    let mut out = Vec::new();
    for f in crate::model::ARTIFACT_TEXT_FIELDS {
        for x in authors_of_artifact_field(a, f) {
            push(&mut out, x);
        }
    }
    out
}

/// An asset's name and bytes: its uploader, and for a page's upload every
/// author of the page at the upload.
pub fn authors_of_asset(a: &Asset) -> Vec<Author> {
    let mut out = vec![Author::of_token(a.person.as_deref())];
    if !a.page_authors.is_empty() {
        for x in page_set(&a.page_authors) {
            push(&mut out, x);
        }
    }
    out
}

/// A document's fields: every stored author (`doc_authors.author`), or the
/// owner when it has none.
pub fn authors_of_doc(tokens: &[String]) -> Vec<Author> {
    page_set(tokens)
}

/// A working record's message: its session's person (`None`: the owner).
pub fn authors_of_working(session_person: Option<&str>) -> Vec<Author> {
    vec![Author::of_token(session_person)]
}

/// What an unconnected author's name reads in every payload and view an
/// agent gets (spec §9.6, §19 item 7): `<name> (not connected)`.
pub const NOT_CONNECTED: &str = " (not connected)";

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
    /// in its `anchor`, and everything else the anchored page gave it with
    /// them (its `custom_name`, the anchored `file`'s name, the `html_hash`
    /// of the anchored element's markup, and its drawn area's `tag`, `text`
    /// and `children` fingerprint, which is left out); the thread marked
    /// `anchor_withheld: true`.
    pub fn anchor(thread: &mut Value) {
        if let Some(anchor) = thread.get_mut("anchor").and_then(Value::as_object_mut) {
            for k in [
                "quote",
                "prefix",
                "suffix",
                "selector",
                "custom_name",
                "file",
                "html_hash",
            ] {
                anchor.insert(k.into(), Value::Null);
            }
            if let Some(area) = anchor.get_mut("area").and_then(Value::as_object_mut) {
                for k in ["tag", "text", "children"] {
                    area.remove(k);
                }
            }
        }
        thread["anchor_withheld"] = json!(true);
    }

    /// A thread's clip: no `clip`, its `clip_url` and `clip_path` (where a
    /// view has them) null and `has_clip` false, so nothing names a clip to
    /// fetch; and `clip_withheld: true`.
    pub fn clip(thread: &mut Value) {
        if let Some(t) = thread.as_object_mut() {
            t.remove("clip");
            for k in ["clip_url", "clip_path"] {
                if let Some(v) = t.get_mut(k) {
                    *v = Value::Null;
                }
            }
            if let Some(v) = t.get_mut("has_clip") {
                *v = json!(false);
            }
        }
        thread["clip_withheld"] = json!(true);
    }

    /// A thread's `resolved_by`: null, with the name a view derives from it
    /// (`resolved_by_name`, where it has one).
    pub fn resolved_by(thread: &mut Value) {
        thread["resolved_by"] = Value::Null;
        if let Some(v) = thread.get_mut("resolved_by_name") {
            *v = Value::Null;
        }
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

    /// An artifact whose text `fields` (of `title`, `description`, `icon`
    /// and `capabilities`) are withheld: each null, and `withheld:
    /// {authors}`, every author they withhold.
    pub fn artifact(artifact: &mut Value, fields: &[&str], authors: &[WithheldAuthor]) {
        for k in fields {
            artifact[*k] = Value::Null;
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
    /// `files_withheld: n`, how many were. When any was, the version's
    /// `content_sha256` (a hash over every file's name and bytes, which
    /// would confirm a guess at them) is null too, where it has one.
    pub fn version_files(version: &mut Value, withheld: &[&str]) {
        let mut n = 0;
        if let Some(files) = version.get_mut("files").and_then(Value::as_object_mut) {
            for path in withheld {
                n += usize::from(files.remove(*path).is_some());
            }
        }
        version["files_withheld"] = json!(n);
        if n > 0
            && let Some(v) = version.get_mut("content_sha256")
        {
            *v = Value::Null;
        }
    }

    /// A document read by its path: `{doc_id, version, withheld:
    /// {authors}}`, without its data.
    pub fn document(doc_id: &str, version: u64, authors: &[WithheldAuthor]) -> Value {
        json!({"doc_id": doc_id, "version": version, "withheld": {"authors": authors_json(authors)}})
    }

    /// An asset: `name` null, and with it what its bytes decide (its
    /// `content_type`, `ext` and `size`, where it has them), and
    /// `withheld: true`. Its ID, artifact and time stay.
    pub fn asset(asset: &mut Value) {
        asset["name"] = Value::Null;
        if let Some(a) = asset.as_object_mut() {
            for k in ["content_type", "ext", "size"] {
                if let Some(v) = a.get_mut(k) {
                    *v = Value::Null;
                }
            }
        }
        asset["withheld"] = json!(true);
    }

    /// An artifact's title where its text is withheld.
    pub fn title_by(name: &str) -> String {
        format!("an artifact by {name}")
    }

    /// The feedback line for `n` withheld comments by `author` in thread
    /// `thread_id` of the artifact titled `title` (as the reader may see
    /// it: [`title_by`] when it is withheld), saying whether the reader's
    /// person has been asked. One comment reads `1 comment … is withheld
    /// until your person approves it`, more `N comments … are withheld
    /// until your person approves them`. The title is quoted as a JSON string and the
    /// name made safe ([`crate::feedback::display_name`]), so neither can
    /// break the line.
    pub fn withheld_comments_line(
        n: usize,
        author: &str,
        title: &str,
        thread_id: &str,
        asked: bool,
    ) -> String {
        let then = if asked {
            "Your person has been asked.".to_string()
        } else {
            format!(
                "To ask your person, call ask with {{\"approval\": {{\"thread_id\": \"{thread_id}\"}}}}."
            )
        };
        let (comments, are, them) = counted(n);
        format!(
            "[clax] {n} {comments} from {author} on {title} (thread {thread_id}) {are} withheld until your person approves {them}. {then}",
            author = crate::feedback::display_name(author),
            title = crate::feedback::quoted(title),
        )
    }

    /// The feedback line for `n` comments in thread `thread_id` held
    /// because the reader may no longer read their artifact (spec §7.5):
    /// nothing of the page or the comments, only the count and the
    /// thread's ID.
    pub fn no_access_line(n: usize, thread_id: &str) -> String {
        let (comments, are, _) = counted(n);
        format!(
            "[clax] {n} {comments} in thread {thread_id} {are} held: you no longer have access to this page."
        )
    }

    /// `comments`, `are` and `them` agreeing with `n`.
    fn counted(n: usize) -> (&'static str, &'static str, &'static str) {
        if n == 1 {
            ("comment", "is", "it")
        } else {
            ("comments", "are", "them")
        }
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

//! Git objects: loose encoding and IDs, trees, commits, and Clax's commit
//! message trailers. The encodings are byte for byte git's, so the IDs equal
//! what `git hash-object`, `git mktree` and `git commit-tree` produce.
//! Clax writes blobs, trees and commits; tags it writes are lightweight
//! (refs only), but annotated tag objects pushed to it are kept and served.

use std::cmp::Ordering;
use std::fmt;

use crate::error::{GitError, Result};
use crate::id::ObjectId;

/// The four git object types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl ObjectKind {
    /// The name in the loose header: `commit`, `tree`, `blob`, `tag`.
    pub fn name(self) -> &'static str {
        match self {
            ObjectKind::Commit => "commit",
            ObjectKind::Tree => "tree",
            ObjectKind::Blob => "blob",
            ObjectKind::Tag => "tag",
        }
    }

    /// The type code in a pack entry header.
    pub fn pack_code(self) -> u8 {
        match self {
            ObjectKind::Commit => 1,
            ObjectKind::Tree => 2,
            ObjectKind::Blob => 3,
            ObjectKind::Tag => 4,
        }
    }

    pub fn from_pack_code(code: u8) -> Option<ObjectKind> {
        match code {
            1 => Some(ObjectKind::Commit),
            2 => Some(ObjectKind::Tree),
            3 => Some(ObjectKind::Blob),
            4 => Some(ObjectKind::Tag),
            _ => None,
        }
    }
}

impl fmt::Display for ObjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A git object: its type and content (without the loose header).
#[derive(Clone, PartialEq, Eq)]
pub struct Object {
    pub kind: ObjectKind,
    pub data: Vec<u8>,
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Object({} {}, {} bytes)",
            self.kind,
            self.id(),
            self.data.len()
        )
    }
}

impl Object {
    pub fn new(kind: ObjectKind, data: Vec<u8>) -> Object {
        Object { kind, data }
    }

    pub fn blob(data: impl Into<Vec<u8>>) -> Object {
        Object::new(ObjectKind::Blob, data.into())
    }

    /// The object's ID: the SHA-1 of `<type> <size>\0<content>`.
    pub fn id(&self) -> ObjectId {
        hash_object(self.kind, &self.data)
    }
}

/// The loose-object form `<type> <size>\0<content>` that git hashes.
pub fn encode_loose(kind: ObjectKind, content: &[u8]) -> Vec<u8> {
    let mut out = format!("{} {}\0", kind.name(), content.len()).into_bytes();
    out.extend_from_slice(content);
    out
}

/// The ID of an object of `kind` holding `content`.
pub fn hash_object(kind: ObjectKind, content: &[u8]) -> ObjectId {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(format!("{} {}\0", kind.name(), content.len()).as_bytes());
    h.update(content);
    ObjectId(h.finalize().into())
}

/// The ID of the empty tree.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

// ── Trees ───────────────────────────────────────────────────────────────────

/// A tree entry's mode, as git writes it in octal (`100644`, `40000`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mode(pub u32);

impl Mode {
    pub const FILE: Mode = Mode(0o100644);
    pub const EXEC: Mode = Mode(0o100755);
    pub const TREE: Mode = Mode(0o40000);
    pub const SYMLINK: Mode = Mode(0o120000);
    pub const GITLINK: Mode = Mode(0o160000);

    pub fn is_tree(self) -> bool {
        self == Mode::TREE
    }

    /// Whether the entry names a blob: a file, an executable or a symlink.
    pub fn is_blob(self) -> bool {
        matches!(self, Mode::FILE | Mode::EXEC | Mode::SYMLINK)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:o}", self.0)
    }
}

/// One entry of a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: Mode,
    pub name: String,
    pub id: ObjectId,
}

/// Git's tree order: names compared as bytes, a tree's name as if it ended
/// in `/`. A file `a-b`, a file `a.c`, a tree `a` and a file `ab` sort in
/// that order.
pub fn compare_tree_entries(a: &TreeEntry, b: &TreeEntry) -> Ordering {
    let ka = a
        .name
        .as_bytes()
        .iter()
        .copied()
        .chain(a.mode.is_tree().then_some(b'/'));
    let kb = b
        .name
        .as_bytes()
        .iter()
        .copied()
        .chain(b.mode.is_tree().then_some(b'/'));
    ka.cmp(kb)
}

/// Tree content: `<mode> <name>\0<20-byte ID>` per entry, in git's order.
/// Refuses a duplicate name (a file and a directory of one name included),
/// an empty one, or one holding `/` or NUL.
pub fn encode_tree(entries: &[TreeEntry]) -> Result<Vec<u8>> {
    let mut sorted: Vec<&TreeEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| compare_tree_entries(a, b));
    // Git's order need not put two entries of one name side by side: a
    // file `a`, a file `a.c` and a directory `a` sort in that order.
    let mut names = std::collections::HashSet::with_capacity(sorted.len());
    let mut out = Vec::new();
    for e in &sorted {
        if e.name.is_empty() || e.name.contains(['/', '\0']) {
            return Err(GitError::invalid(format!(
                "Not a valid tree entry name: {:?}",
                e.name
            )));
        }
        if !names.insert(e.name.as_str()) {
            return Err(GitError::invalid(format!(
                "Duplicate tree entry {:?}",
                e.name
            )));
        }
        out.extend_from_slice(format!("{} {}\0", e.mode, e.name).as_bytes());
        out.extend_from_slice(e.id.as_bytes());
    }
    Ok(out)
}

/// The entries of a tree's content, in stored order.
pub fn parse_tree(content: &[u8]) -> Result<Vec<TreeEntry>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < content.len() {
        let malformed = || GitError::unavailable("A tree object is malformed");
        let space = content[at..]
            .iter()
            .position(|&b| b == b' ')
            .ok_or_else(malformed)?
            + at;
        let nul = content[space + 1..]
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(malformed)?
            + space
            + 1;
        if nul + 21 > content.len() {
            return Err(malformed());
        }
        let mode = std::str::from_utf8(&content[at..space])
            .ok()
            .and_then(|m| u32::from_str_radix(m, 8).ok())
            .ok_or_else(malformed)?;
        let name = String::from_utf8_lossy(&content[space + 1..nul]).into_owned();
        let id = ObjectId::from_bytes(&content[nul + 1..nul + 21]).ok_or_else(malformed)?;
        out.push(TreeEntry {
            mode: Mode(mode),
            name,
            id,
        });
        at = nul + 21;
    }
    Ok(out)
}

// ── Commits ─────────────────────────────────────────────────────────────────

/// A commit author or committer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub email: String,
}

/// A person and the moment of their signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub email: String,
    /// Unix seconds.
    pub time: i64,
    /// Minutes east of UTC; 0 writes `+0000`.
    pub tz_offset_minutes: i32,
}

impl Signature {
    /// `person` at `time` (unix seconds), in UTC.
    pub fn utc(person: &Person, time: i64) -> Signature {
        Signature {
            name: person.name.clone(),
            email: person.email.clone(),
            time,
            tz_offset_minutes: 0,
        }
    }

    fn format(&self) -> Result<String> {
        for v in [&self.name, &self.email] {
            if v.contains(['<', '>', '\n', '\0']) {
                return Err(GitError::invalid(format!(
                    "A commit author may not contain <, >, a newline or NUL: {v:?}"
                )));
            }
        }
        let m = self.tz_offset_minutes;
        let sign = if m < 0 { '-' } else { '+' };
        let abs = m.unsigned_abs();
        Ok(format!(
            "{} <{}> {} {sign}{:02}{:02}",
            self.name.trim(),
            self.email.trim(),
            self.time,
            abs / 60,
            abs % 60
        ))
    }

    fn parse(value: &str) -> Signature {
        let fallback = || Signature {
            name: value.to_string(),
            email: String::new(),
            time: 0,
            tz_offset_minutes: 0,
        };
        let Some(lt) = value.find('<') else {
            return fallback();
        };
        let Some(gt) = value[lt..].find('>').map(|i| i + lt) else {
            return fallback();
        };
        let name = value[..lt].strip_suffix(' ').unwrap_or(&value[..lt]);
        let email = &value[lt + 1..gt];
        let rest = &value[gt + 1..];
        let mut parts = rest.strip_prefix(' ').unwrap_or("").split(' ');
        let (Some(time), Some(tz), None) = (parts.next(), parts.next(), parts.next()) else {
            return fallback();
        };
        let Ok(time) = time.parse::<i64>() else {
            return fallback();
        };
        let tz_offset_minutes = parse_tz(tz).unwrap_or(0);
        Signature {
            name: name.to_string(),
            email: email.to_string(),
            time,
            tz_offset_minutes,
        }
    }
}

fn parse_tz(tz: &str) -> Option<i32> {
    let b = tz.as_bytes();
    if b.len() != 5 || !matches!(b[0], b'+' | b'-') || !b[1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let minutes = tz[1..3].parse::<i32>().ok()? * 60 + tz[3..5].parse::<i32>().ok()?;
    Some(if b[0] == b'-' { -minutes } else { minutes })
}

/// A commit's fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub tree: ObjectId,
    /// In git order; empty for a root commit.
    pub parents: Vec<ObjectId>,
    pub author: Signature,
    pub committer: Signature,
    /// Stored verbatim; [`Commit::encode`] adds a missing final newline, as
    /// `git commit-tree -m` does.
    pub message: String,
}

/// `message`, ending in a newline.
pub fn normalize_message(message: &str) -> String {
    if message.ends_with('\n') {
        message.to_string()
    } else {
        format!("{message}\n")
    }
}

impl Commit {
    /// The commit object's content. Refuses a NUL in the message and `<`,
    /// `>`, a newline or NUL in a name or email.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.message.contains('\0') {
            return Err(GitError::invalid("A commit message may not contain NUL"));
        }
        let mut text = format!("tree {}\n", self.tree);
        for p in &self.parents {
            text.push_str(&format!("parent {p}\n"));
        }
        text.push_str(&format!("author {}\n", self.author.format()?));
        text.push_str(&format!("committer {}\n", self.committer.format()?));
        text.push('\n');
        text.push_str(&normalize_message(&self.message));
        Ok(text.into_bytes())
    }

    /// The commit as an object.
    pub fn to_object(&self) -> Result<Object> {
        Ok(Object::new(ObjectKind::Commit, self.encode()?))
    }

    /// Parses a commit, skipping headers it does not use (`gpgsig`,
    /// `encoding`, …). Text that is not UTF-8 is read lossily.
    pub fn parse(content: &[u8]) -> Result<Commit> {
        let text = String::from_utf8_lossy(content);
        let (head, message) = match text.find("\n\n") {
            Some(i) => (&text[..i], &text[i + 2..]),
            None => (&text[..], ""),
        };
        let mut tree = None;
        let mut parents = Vec::new();
        let blank = Signature {
            name: String::new(),
            email: String::new(),
            time: 0,
            tz_offset_minutes: 0,
        };
        let mut author = blank.clone();
        let mut committer = blank;
        for line in head.split('\n') {
            if line.starts_with(' ') {
                continue;
            }
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "tree" => tree = ObjectId::from_hex(value),
                "parent" => {
                    parents.push(ObjectId::from_hex(value).ok_or_else(|| {
                        GitError::unavailable("A commit names a malformed parent")
                    })?);
                }
                "author" => author = Signature::parse(value),
                "committer" => committer = Signature::parse(value),
                _ => {}
            }
        }
        let tree = tree.ok_or_else(|| GitError::unavailable("A commit has no tree"))?;
        Ok(Commit {
            tree,
            parents,
            author,
            committer,
            message: message.to_string(),
        })
    }
}

/// The object an annotated tag names, from the tag object's content.
pub fn tag_target(content: &[u8]) -> Option<(ObjectId, ObjectKind)> {
    let text = std::str::from_utf8(content).ok()?;
    let mut id = None;
    let mut kind = None;
    for line in text.split('\n') {
        if line.is_empty() {
            break;
        }
        match line.split_once(' ') {
            Some(("object", v)) => id = ObjectId::from_hex(v),
            Some(("type", v)) => {
                kind = match v {
                    "commit" => Some(ObjectKind::Commit),
                    "tree" => Some(ObjectKind::Tree),
                    "blob" => Some(ObjectKind::Blob),
                    "tag" => Some(ObjectKind::Tag),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    Some((id?, kind?))
}

// ── Messages and trailers ───────────────────────────────────────────────────

fn is_trailer_key(k: &str) -> bool {
    let b = k.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
}

fn parse_trailer_line(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once(": ")?;
    is_trailer_key(k).then(|| (k.to_string(), v.to_string()))
}

/// `note` followed by a paragraph of `Key: value` trailers (Clax writes
/// `Clax-Version`, `Clax-Publisher`, `Clax-Agent`, `Clax-Forked-From`). An
/// empty note makes a message of trailers alone. Refuses a key that is not
/// letters, digits and `-` (starting with a letter or digit), and a value
/// holding a newline.
pub fn with_trailers(note: &str, trailers: &[(String, String)]) -> Result<String> {
    let body = note.trim_end();
    if trailers.is_empty() {
        return Ok(normalize_message(body));
    }
    let mut block = Vec::new();
    for (k, v) in trailers {
        if !is_trailer_key(k) || v.contains(['\n', '\r']) {
            return Err(GitError::invalid(format!("Bad trailer {k}")));
        }
        block.push(format!("{k}: {v}"));
    }
    let block = block.join("\n");
    Ok(if body.is_empty() {
        format!("{block}\n")
    } else {
        format!("{body}\n\n{block}\n")
    })
}

/// The trailers of a message: its last paragraph, when every line of it is
/// `Key: value`; else none.
pub fn parse_trailers(message: &str) -> Vec<(String, String)> {
    let trimmed = message.trim_end();
    let lines: Vec<&str> = trimmed.split('\n').collect();
    let start = lines
        .iter()
        .rposition(|l| l.trim().is_empty())
        .map_or(0, |i| i + 1);
    let last = &lines[start..];
    if last.is_empty() || (last.len() == 1 && last[0].is_empty()) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in last {
        match parse_trailer_line(line) {
            Some(t) => out.push(t),
            None => return Vec::new(),
        }
    }
    out
}

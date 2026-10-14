//! Building a new tree from a parent tree and a set of path changes,
//! creating only the blobs that changed and the trees on changed paths
//! (unchanged subtrees keep their ID and are not read), and walking trees
//! by path.

use std::collections::{BTreeMap, HashMap};

use crate::error::{GitError, Result};
use crate::id::ObjectId;
use crate::object::{Mode, Object, ObjectKind, TreeEntry, encode_tree, parse_tree};
use crate::repo::Repo;

/// Paths (slash-separated, relative) to the bytes they now hold; `None`
/// deletes the path.
pub type Changes = BTreeMap<String, Option<Vec<u8>>>;

/// Splits a repository path into segments, refusing ones git or a checkout
/// would not accept: empty, `.`, `..`, `.git` in any case, a NUL or a
/// backslash.
pub fn path_segments(path: &str) -> Result<Vec<&str>> {
    let segments: Vec<&str> = path.split('/').collect();
    for s in &segments {
        if s.is_empty()
            || *s == "."
            || *s == ".."
            || s.eq_ignore_ascii_case(".git")
            || s.contains(['\0', '\\'])
        {
            return Err(GitError::invalid(format!(
                "Not a valid file path: {path:?}"
            )));
        }
    }
    Ok(segments)
}

/// The entries of tree `id` in `repo`, or `None` when it holds no such tree.
pub async fn read_tree(repo: &impl Repo, id: &ObjectId) -> Result<Option<Vec<TreeEntry>>> {
    match repo.read_object(id).await? {
        Some(o) if o.kind == ObjectKind::Tree => Ok(Some(parse_tree(&o.data)?)),
        _ => Ok(None),
    }
}

#[derive(Default)]
struct Node<'a> {
    files: BTreeMap<&'a str, Option<&'a [u8]>>,
    dirs: BTreeMap<&'a str, Node<'a>>,
}

fn trie(changes: &Changes) -> Result<Node<'_>> {
    let mut root = Node::default();
    for (path, bytes) in changes {
        let segments = path_segments(path)?;
        let (name, dirs) = segments
            .split_last()
            .expect("split yields one segment at least");
        let mut node = &mut root;
        for dir in dirs {
            if node.files.contains_key(dir) {
                return Err(GitError::invalid(format!(
                    "{path:?} is under a path the same publish names as a file"
                )));
            }
            node = node.dirs.entry(dir).or_default();
        }
        if node.dirs.contains_key(name) {
            return Err(GitError::invalid(format!(
                "{path:?} is also a directory in the same publish"
            )));
        }
        node.files.insert(name, bytes.as_deref());
    }
    Ok(root)
}

async fn existing(repo: &impl Repo, id: Option<ObjectId>) -> Result<Vec<TreeEntry>> {
    let Some(id) = id else { return Ok(Vec::new()) };
    read_tree(repo, &id)
        .await?
        .ok_or_else(|| GitError::unavailable(format!("Tree {id} is missing from the repository")))
}

/// Reads the base tree of every directory the changes touch, top down,
/// keyed by directory path (`""` the root, else `a/b/`). Iterative, so the
/// future stays unboxed (and `Send` when the repository's are).
async fn load_bases<'a>(
    repo: &impl Repo,
    root: &'a Node<'a>,
    base: Option<ObjectId>,
) -> Result<HashMap<String, Vec<TreeEntry>>> {
    let mut out = HashMap::new();
    let mut stack: Vec<(String, &'a Node<'a>, Option<ObjectId>)> =
        vec![(String::new(), root, base)];
    while let Some((key, node, base)) = stack.pop() {
        let entries = existing(repo, base).await?;
        for (name, child) in &node.dirs {
            let child_base = entries
                .iter()
                .find(|e| e.name == *name && e.mode.is_tree())
                .map(|e| e.id);
            stack.push((format!("{key}{name}/"), child, child_base));
        }
        out.insert(key, entries);
    }
    Ok(out)
}

/// The entries of `node`'s directory (`key`) with its changes applied,
/// pushing new objects to `out`.
fn build(
    node: &Node<'_>,
    key: &str,
    bases: &HashMap<String, Vec<TreeEntry>>,
    out: &mut Vec<Object>,
) -> Result<Vec<TreeEntry>> {
    let mut entries: BTreeMap<String, TreeEntry> = bases
        .get(key)
        .into_iter()
        .flatten()
        .map(|e| (e.name.clone(), e.clone()))
        .collect();
    for (name, bytes) in &node.files {
        let Some(bytes) = bytes else {
            entries.remove(*name);
            continue;
        };
        let blob = Object::blob(bytes.to_vec());
        let id = blob.id();
        out.push(blob);
        // A path that was a directory becomes a file; an executable stays one.
        let mode = match entries.get(*name) {
            Some(e) if e.mode == Mode::EXEC => Mode::EXEC,
            _ => Mode::FILE,
        };
        entries.insert(
            name.to_string(),
            TreeEntry {
                mode,
                name: name.to_string(),
                id,
            },
        );
    }
    for (name, child) in &node.dirs {
        let child_entries = build(child, &format!("{key}{name}/"), bases, out)?;
        if child_entries.is_empty() {
            entries.remove(*name);
            continue;
        }
        let tree = Object::new(ObjectKind::Tree, encode_tree(&child_entries)?);
        let id = tree.id();
        out.push(tree);
        entries.insert(
            name.to_string(),
            TreeEntry {
                mode: Mode::TREE,
                name: name.to_string(),
                id,
            },
        );
    }
    Ok(entries.into_values().collect())
}

/// The tree of `base` with `changes` applied (`None` deletes; a directory
/// left empty disappears), and the objects it needs that may be new: the
/// blobs written and the trees on changed paths, root tree last.
pub async fn build_tree(
    repo: &impl Repo,
    base: Option<ObjectId>,
    changes: &Changes,
) -> Result<(ObjectId, Vec<Object>)> {
    let node = trie(changes)?;
    let bases = load_bases(repo, &node, base).await?;
    let mut out = Vec::new();
    let entries = build(&node, "", &bases, &mut out)?;
    let root = Object::new(ObjectKind::Tree, encode_tree(&entries)?);
    let id = root.id();
    out.push(root);
    dedup(&mut out);
    Ok((id, out))
}

fn dedup(objects: &mut Vec<Object>) {
    let mut seen = std::collections::HashSet::new();
    objects.retain(|o| seen.insert(o.id()));
}

/// One blob-like entry below a tree: its full path, mode and ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathEntry {
    pub path: String,
    pub mode: Mode,
    pub id: ObjectId,
}

/// Every non-tree entry below `tree` (files, symlinks, submodules), with
/// full paths, in tree order.
pub async fn walk_tree(repo: &impl Repo, tree: &ObjectId) -> Result<Vec<PathEntry>> {
    let mut out = Vec::new();
    let mut stack = vec![(String::new(), *tree)];
    while let Some((prefix, id)) = stack.pop() {
        let entries = existing(repo, Some(id)).await?;
        let mut subtrees = Vec::new();
        for e in entries {
            let path = format!("{prefix}{}", e.name);
            if e.mode.is_tree() {
                subtrees.push((format!("{path}/"), e.id));
            } else {
                out.push(PathEntry {
                    path,
                    mode: e.mode,
                    id: e.id,
                });
            }
        }
        stack.extend(subtrees.into_iter().rev());
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The entry at `path` below `tree`, or `None`.
pub async fn resolve_path(
    repo: &impl Repo,
    tree: &ObjectId,
    path: &str,
) -> Result<Option<TreeEntry>> {
    let Ok(segments) = path_segments(path) else {
        return Ok(None);
    };
    let mut current = *tree;
    for (i, seg) in segments.iter().enumerate() {
        let Some(entries) = read_tree(repo, &current).await? else {
            return Ok(None);
        };
        let Some(e) = entries.into_iter().find(|e| e.name == *seg) else {
            return Ok(None);
        };
        if i == segments.len() - 1 {
            return Ok(Some(e));
        }
        if !e.mode.is_tree() {
            return Ok(None);
        }
        current = e.id;
    }
    Ok(None)
}

//! Objects, trees, commits and packs against the real `git`: IDs equal
//! `git hash-object`, `mktree`, `write-tree` and `commit-tree`; a pack we
//! write passes `index-pack` and `verify-pack`; packs git writes (deltified,
//! and thin) read back to the objects git holds.

use std::collections::HashMap;

use clax_git::memory::MemoryHost;
use clax_git::object::{
    EMPTY_TREE, encode_tree, hash_object, parse_trailers, parse_tree, with_trailers,
};
use clax_git::pack::{DEFAULT_MAX_OBJECT_BYTES, ParsedPack, Resolved, write_pack};
use clax_git::tree::{Changes, build_tree};
use clax_git::{
    Commit, Mode, Object, ObjectId, ObjectKind, Person, RepoHost, Signature, TreeEntry,
};

use crate::common::{Home, temp};

fn bare_repo(home: &Home) -> tempfile::TempDir {
    let dir = temp("objects");
    home.git(dir.path(), &["init", "-q", "-b", "main"]);
    dir
}

#[test]
fn blobs_hash_as_git_hash_object() {
    let home = Home::new();
    let repo = bare_repo(&home);
    let big = "x".repeat(70000);
    for content in ["", "hello\n", "ünïcode ✓", big.as_str()] {
        let ours = hash_object(ObjectKind::Blob, content.as_bytes()).to_hex();
        let theirs = String::from_utf8(home.git_with(
            repo.path(),
            &["hash-object", "--stdin"],
            &[],
            Some(content.as_bytes()),
        ))
        .unwrap();
        assert_eq!(ours, theirs.trim());
    }
    assert_eq!(hash_object(ObjectKind::Tree, b"").to_hex(), EMPTY_TREE);
}

#[test]
fn trees_encode_in_git_order_as_git_mktree() {
    let home = Home::new();
    let repo = bare_repo(&home);
    let blob = hash_object(ObjectKind::Blob, b"x");
    let sub = hash_object(ObjectKind::Tree, b"");
    let e = |name: &str, mode| TreeEntry {
        mode,
        name: name.into(),
        id: if mode == Mode::TREE { sub } else { blob },
    };
    let entries = vec![
        e("ab", Mode::FILE),
        e("a", Mode::TREE),
        e("a.c", Mode::FILE),
        e("a-b", Mode::FILE),
        e("Z", Mode::FILE),
        e("run", Mode::EXEC),
    ];
    let content = encode_tree(&entries).unwrap();
    let input: String = entries
        .iter()
        .map(|e| {
            let kind = if e.mode == Mode::TREE {
                "040000 tree"
            } else if e.mode == Mode::EXEC {
                "100755 blob"
            } else {
                "100644 blob"
            };
            format!("{kind} {}\t{}\n", e.id, e.name)
        })
        .collect();
    let theirs = String::from_utf8(home.git_with(
        repo.path(),
        &["mktree", "--missing"],
        &[],
        Some(input.as_bytes()),
    ))
    .unwrap();
    assert_eq!(
        hash_object(ObjectKind::Tree, &content).to_hex(),
        theirs.trim()
    );
    let names: Vec<String> = parse_tree(&content)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["Z", "a-b", "a.c", "a", "ab", "run"]);
    assert!(encode_tree(&[e("a", Mode::FILE), e("a", Mode::FILE)]).is_err());
    // Git order puts `a.c` between the file `a` and the directory `a/`.
    let err =
        encode_tree(&[e("a", Mode::FILE), e("a.c", Mode::FILE), e("a", Mode::TREE)]).unwrap_err();
    assert!(err.to_string().contains("Duplicate"), "{err}");
}

#[tokio::test]
async fn build_tree_matches_git_write_tree() {
    let home = Home::new();
    let work = temp("worktree");
    home.git(work.path(), &["init", "-q", "-b", "main"]);
    let files = [
        ("index.html", "<h1>hi</h1>\n"),
        ("a-b", "1"),
        ("a.c", "2"),
        ("a/x.txt", "3"),
        ("a/deep/y.txt", "4"),
        ("ab", "5"),
        ("empty.txt", ""),
    ];
    for (p, c) in files {
        let path = work.path().join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, c).unwrap();
    }
    home.git(work.path(), &["add", "-A"]);
    let expected = home.git_text(work.path(), &["write-tree"]);

    let host = MemoryHost::new();
    host.create("r").await.unwrap();
    let repo = host.repo("r").unwrap();
    let changes: Changes = files
        .iter()
        .map(|(p, c)| (p.to_string(), Some(c.as_bytes().to_vec())))
        .collect();
    let (tree, objects) = build_tree(&repo, None, &changes).await.unwrap();
    assert_eq!(tree.to_hex(), expected);
    // Every object made is the object git stored under that ID.
    for o in &objects {
        let theirs = home.git(work.path(), &["cat-file", o.kind.name(), &o.id().to_hex()]);
        assert_eq!(theirs, o.data, "{}", o.id());
    }
}

#[test]
fn commits_match_git_commit_tree_and_trailers_parse_as_git_does() {
    let home = Home::new();
    let repo = bare_repo(&home);
    let tree = home.git_text_input(
        repo.path(),
        &["hash-object", "-w", "-t", "tree", "--stdin"],
        b"",
    );
    let tree = ObjectId::from_hex(&tree).unwrap();
    let trailers = [
        ("Clax-Version", "2"),
        ("Clax-Publisher", "0123456789ab"),
        ("Clax-Agent", "claude"),
    ]
    .map(|(k, v)| (k.to_string(), v.to_string()));
    let message = with_trailers("A note\n\nwith a body", &trailers).unwrap();
    let msg_dir = temp("msg");
    let msg_file = msg_dir.path().join("msg");
    std::fs::write(&msg_file, &message).unwrap();
    let env = [
        ("GIT_AUTHOR_NAME", "Mia Example"),
        ("GIT_AUTHOR_EMAIL", "mia@clax.dev"),
        ("GIT_AUTHOR_DATE", "@1760140800 +0000"),
        ("GIT_COMMITTER_NAME", "Mia Example"),
        ("GIT_COMMITTER_EMAIL", "mia@clax.dev"),
        ("GIT_COMMITTER_DATE", "@1760140800 +0000"),
    ];
    let msg = msg_file.to_str().unwrap();
    let root = String::from_utf8(home.git_with(
        repo.path(),
        &["commit-tree", &tree.to_hex(), "-F", msg],
        &env,
        None,
    ))
    .unwrap();
    let root = ObjectId::from_hex(root.trim()).unwrap();
    let sig = Signature::utc(
        &Person {
            name: "Mia Example".into(),
            email: "mia@clax.dev".into(),
        },
        1760140800,
    );
    let c = Commit {
        tree,
        parents: vec![],
        author: sig.clone(),
        committer: sig.clone(),
        message: message.clone(),
    };
    assert_eq!(c.to_object().unwrap().id(), root);

    let child = String::from_utf8(home.git_with(
        repo.path(),
        &[
            "commit-tree",
            &tree.to_hex(),
            "-p",
            &root.to_hex(),
            "-F",
            msg,
        ],
        &env,
        None,
    ))
    .unwrap();
    let c2 = Commit {
        parents: vec![root],
        ..c
    };
    let content = c2.encode().unwrap();
    assert_eq!(
        hash_object(ObjectKind::Commit, &content).to_hex(),
        child.trim()
    );
    assert_eq!(
        home.git(repo.path(), &["cat-file", "commit", child.trim()]),
        content
    );
    let parsed = Commit::parse(&content).unwrap();
    assert_eq!(parsed, c2);
    assert_eq!(parse_trailers(&parsed.message), trailers.to_vec());
    let git_trailers = String::from_utf8(home.git_with(
        repo.path(),
        &["interpret-trailers", "--parse"],
        &[],
        Some(message.as_bytes()),
    ))
    .unwrap();
    assert_eq!(
        git_trailers.trim(),
        "Clax-Version: 2\nClax-Publisher: 0123456789ab\nClax-Agent: claude"
    );

    // Messages of trailers alone, and of a note alone.
    assert_eq!(
        with_trailers("", &trailers[..1]).unwrap(),
        "Clax-Version: 2\n"
    );
    assert_eq!(with_trailers("note  \n", &[]).unwrap(), "note\n");
    assert!(parse_trailers("just a note").is_empty());
    assert!(with_trailers("x", &[("Bad Key".into(), "v".into())]).is_err());
    let nul = Commit {
        message: "a\0b".into(),
        ..c2.clone()
    };
    assert!(nul.encode().is_err());
}

#[tokio::test]
async fn packs_we_write_pass_index_pack_and_verify_pack() {
    let home = Home::new();
    let repo = bare_repo(&home);
    let host = MemoryHost::new();
    host.create("r").await.unwrap();
    let r = host.repo("r").unwrap();
    let changes: Changes = [("a/b.txt", "b".repeat(5000)), ("c.txt", "c".into())]
        .into_iter()
        .map(|(p, c)| (p.to_string(), Some(c.into_bytes())))
        .collect();
    let (tree, mut objects) = build_tree(&r, None, &changes).await.unwrap();
    let sig = Signature::utc(
        &Person {
            name: "M".into(),
            email: "m@x".into(),
        },
        0,
    );
    let commit = Commit {
        tree,
        parents: vec![],
        author: sig.clone(),
        committer: sig,
        message: "m".into(),
    }
    .to_object()
    .unwrap();
    objects.push(commit.clone());
    let dir = temp("pack");
    let pack_path = dir.path().join("test.pack");
    std::fs::write(&pack_path, write_pack(&objects)).unwrap();
    home.git(repo.path(), &["index-pack", pack_path.to_str().unwrap()]);
    let verify = String::from_utf8(home.git(
        repo.path(),
        &[
            "verify-pack",
            "-v",
            dir.path().join("test.idx").to_str().unwrap(),
        ],
    ))
    .unwrap();
    for o in &objects {
        assert!(
            verify.contains(&format!("{} {}", o.id(), o.kind)),
            "{verify}"
        );
    }
    assert!(verify.contains(&commit.id().to_hex()));
    // And our reader reads it back.
    let parsed = ParsedPack::parse(
        &std::fs::read(&pack_path).unwrap(),
        DEFAULT_MAX_OBJECT_BYTES,
    )
    .unwrap();
    let Resolved::Objects(back) = parsed
        .resolve(&HashMap::new(), DEFAULT_MAX_OBJECT_BYTES)
        .unwrap()
    else {
        panic!("missing bases")
    };
    assert_eq!(back, objects);
}

/// A repository with history whose objects git deltifies: two versions of a
/// big file.
fn deltified_repo(home: &Home) -> (tempfile::TempDir, String, String) {
    let work = temp("delta");
    home.git(work.path(), &["init", "-q", "-b", "main"]);
    let base: String = (0..4000)
        .map(|i| format!("line {i} of a long file\n"))
        .collect();
    std::fs::write(work.path().join("big.txt"), &base).unwrap();
    std::fs::write(work.path().join("small.txt"), "small\n").unwrap();
    home.git(work.path(), &["add", "-A"]);
    home.git(
        work.path(),
        &[
            "-c",
            "user.name=R",
            "-c",
            "user.email=r@x",
            "commit",
            "-qm",
            "one",
        ],
    );
    let first = home.git_text(work.path(), &["rev-parse", "HEAD"]);
    std::fs::write(
        work.path().join("big.txt"),
        format!("{base}one more line\n"),
    )
    .unwrap();
    home.git(
        work.path(),
        &[
            "-c",
            "user.name=R",
            "-c",
            "user.email=r@x",
            "commit",
            "-qam",
            "two",
        ],
    );
    let second = home.git_text(work.path(), &["rev-parse", "HEAD"]);
    (work, first, second)
}

fn object_of(home: &Home, dir: &std::path::Path, id: &str) -> Object {
    let kind = home.git_text(dir, &["cat-file", "-t", id]);
    let kind = match kind.as_str() {
        "commit" => ObjectKind::Commit,
        "tree" => ObjectKind::Tree,
        "blob" => ObjectKind::Blob,
        _ => ObjectKind::Tag,
    };
    Object::new(kind, home.git(dir, &["cat-file", kind.name(), id]))
}

#[test]
fn packs_git_writes_with_deltas_read_back_to_git_objects() {
    let home = Home::new();
    let (work, _, second) = deltified_repo(&home);
    let pack = home.git_with(
        work.path(),
        &["pack-objects", "--stdout", "--revs", "--delta-base-offset"],
        &[],
        Some(format!("{second}\n").as_bytes()),
    );
    let parsed = ParsedPack::parse(&pack, DEFAULT_MAX_OBJECT_BYTES).unwrap();
    let Resolved::Objects(objects) = parsed
        .resolve(&HashMap::new(), DEFAULT_MAX_OBJECT_BYTES)
        .unwrap()
    else {
        panic!("missing bases")
    };
    let listed = home.git_text(work.path(), &["rev-list", "--objects", &second]);
    assert_eq!(objects.len(), listed.lines().count());
    for o in &objects {
        assert_eq!(o, &object_of(&home, work.path(), &o.id().to_hex()));
    }
    // git did deltify something (the second big.txt against the first).
    let pack_dir = temp("verify");
    let p = pack_dir.path().join("p.pack");
    std::fs::write(&p, &pack).unwrap();
    home.git(work.path(), &["index-pack", p.to_str().unwrap()]);
    let verify = home.git_text(
        work.path(),
        &[
            "verify-pack",
            "-v",
            pack_dir.path().join("p.idx").to_str().unwrap(),
        ],
    );
    assert!(verify.contains("chain length = 1"), "{verify}");
}

#[test]
fn thin_packs_resolve_against_bases_the_caller_supplies() {
    let home = Home::new();
    let (work, first, second) = deltified_repo(&home);
    let revs = format!("{second}\n^{first}\n");
    let pack = home.git_with(
        work.path(),
        &["pack-objects", "--stdout", "--revs", "--thin"],
        &[],
        Some(revs.as_bytes()),
    );
    let parsed = ParsedPack::parse(&pack, DEFAULT_MAX_OBJECT_BYTES).unwrap();
    let Resolved::Missing(missing) = parsed
        .resolve(&HashMap::new(), DEFAULT_MAX_OBJECT_BYTES)
        .unwrap()
    else {
        panic!("a thin pack names a base outside it")
    };
    let external: HashMap<ObjectId, Object> = missing
        .iter()
        .map(|id| (*id, object_of(&home, work.path(), &id.to_hex())))
        .collect();
    let Resolved::Objects(objects) = parsed.resolve(&external, DEFAULT_MAX_OBJECT_BYTES).unwrap()
    else {
        panic!("still missing")
    };
    let listed = home.git_text(
        work.path(),
        &["rev-list", "--objects", &second, &format!("^{first}")],
    );
    let mut ids: Vec<String> = objects.iter().map(|o| o.id().to_hex()).collect();
    ids.sort();
    let mut want: Vec<String> = listed
        .lines()
        .map(|l| l.split(' ').next().unwrap().to_string())
        .collect();
    want.sort();
    assert_eq!(ids, want);
    for o in &objects {
        assert_eq!(o, &object_of(&home, work.path(), &o.id().to_hex()));
    }
}

#[test]
fn corrupt_packs_are_refused() {
    let pack = write_pack(&[Object::blob(b"hello".to_vec())]);
    let mut bad = pack.clone();
    let n = bad.len();
    bad[n - 1] ^= 1;
    assert!(ParsedPack::parse(&bad, DEFAULT_MAX_OBJECT_BYTES).is_err());
    assert!(ParsedPack::parse(&pack[..20], DEFAULT_MAX_OBJECT_BYTES).is_err());
    assert!(
        ParsedPack::parse(&pack, 2).is_err(),
        "an object over the size limit"
    );
}

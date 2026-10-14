//! Input a pusher controls, built byte by byte: packs whose deltas chain in
//! reverse order, amplify past the unpacked-size limit or name a base that
//! another delta of the same thin pack produces; pushed commits and trees
//! that name an object of the wrong type; and wants a client sends after
//! the ref it read moved on.

use std::collections::HashMap;

use clax_git::memory::{MemoryHost, MemoryRepo};
use clax_git::object::{Signature, encode_tree};
use clax_git::pack::{DEFAULT_MAX_OBJECT_BYTES, ParsedPack, Resolved, deflate_zlib};
use clax_git::pktline::{self, FLUSH};
use clax_git::server::{ReceiveOptions, receive_pack, upload_pack};
use clax_git::{Commit, MAIN, Mode, Object, ObjectId, ObjectKind, RefUpdate, Repo, TreeEntry};

// ── Building packs by hand ──────────────────────────────────────────────────

fn size_header(code: u8, size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut byte = (code << 4) | (size & 0x0f) as u8;
    let mut rest = size >> 4;
    while rest > 0 {
        out.push(byte | 0x80);
        byte = (rest & 0x7f) as u8;
        rest >>= 7;
    }
    out.push(byte);
    out
}

/// One pack entry, as written.
enum Raw {
    Whole(Object),
    RefDelta(ObjectId, Vec<u8>),
}

fn pack_of(entries: &[Raw]) -> Vec<u8> {
    let mut out = b"PACK".to_vec();
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for e in entries {
        match e {
            Raw::Whole(o) => {
                out.extend(size_header(o.kind.pack_code(), o.data.len()));
                out.extend(deflate_zlib(&o.data));
            }
            Raw::RefDelta(base, delta) => {
                out.extend(size_header(7, delta.len()));
                out.extend_from_slice(base.as_bytes());
                out.extend(deflate_zlib(delta));
            }
        }
    }
    let sum = ObjectId::sha1(&out);
    out.extend_from_slice(sum.as_bytes());
    out
}

fn varint(out: &mut Vec<u8>, mut n: usize) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// A delta that copies all of a `base_len`-byte base and appends `tail`.
fn append_delta(base_len: usize, tail: &[u8]) -> Vec<u8> {
    let mut d = Vec::new();
    varint(&mut d, base_len);
    varint(&mut d, base_len + tail.len());
    let mut at = 0;
    while at < base_len {
        let n = (base_len - at).min(0xffffff);
        let mut op = 0x80u8;
        let mut args = Vec::new();
        for i in 0..4 {
            let b = ((at >> (8 * i)) & 0xff) as u8;
            if b != 0 {
                op |= 1 << i;
                args.push(b);
            }
        }
        for i in 0..3 {
            let b = ((n >> (8 * i)) & 0xff) as u8;
            if b != 0 {
                op |= 0x10 << i;
                args.push(b);
            }
        }
        d.push(op);
        d.extend(args);
        at += n;
    }
    for chunk in tail.chunks(127) {
        d.push(chunk.len() as u8);
        d.extend_from_slice(chunk);
    }
    d
}

// ── Pack reading ────────────────────────────────────────────────────────────

/// A chain of deltas, each on the one before it, written newest first: each
/// resolves only after every entry written after it.
#[test]
fn a_ref_delta_chain_written_newest_first_resolves() {
    const N: usize = 500;
    let mut objects = vec![Object::blob(b"0".to_vec())];
    let mut deltas = Vec::new();
    for i in 1..=N {
        let prev = objects.last().unwrap();
        let tail = format!(",{i}");
        deltas.push(Raw::RefDelta(
            prev.id(),
            append_delta(prev.data.len(), tail.as_bytes()),
        ));
        let mut data = prev.data.clone();
        data.extend_from_slice(tail.as_bytes());
        objects.push(Object::blob(data));
    }
    deltas.reverse();
    deltas.push(Raw::Whole(objects[0].clone()));
    let pack = pack_of(&deltas);
    let parsed = ParsedPack::parse(&pack, DEFAULT_MAX_OBJECT_BYTES).unwrap();
    let Resolved::Objects(back) = parsed
        .resolve(&HashMap::new(), DEFAULT_MAX_OBJECT_BYTES)
        .unwrap()
    else {
        panic!("every base is in the pack")
    };
    // Pack order: the newest first, the whole base last.
    let mut want = objects.clone();
    want.reverse();
    assert_eq!(back, want);
}

/// Entries inflating past the pack's unpacked limit together, and deltas that
/// each copy a big base whole, are refused rather than held.
#[test]
fn a_pack_unpacking_past_its_total_limit_is_refused() {
    const K: usize = 64 * 1024;
    let big = Object::blob(vec![b'x'; K]);
    // Four whole 64 KiB blobs against a 192 KiB limit.
    let wholes: Vec<Raw> = (0..4u8)
        .map(|i| {
            let mut d = big.data.clone();
            d[0] = i;
            Raw::Whole(Object::blob(d))
        })
        .collect();
    let pack = pack_of(&wholes);
    let err = ParsedPack::parse_bounded(&pack, DEFAULT_MAX_OBJECT_BYTES, 3 * K).unwrap_err();
    assert!(err.to_string().contains("unpacks to over"), "{err}");
    assert!(ParsedPack::parse_bounded(&pack, DEFAULT_MAX_OBJECT_BYTES, 4 * K).is_ok());

    // One 64 KiB blob and 16 deltas of a few bytes, each a new 64 KiB object.
    let mut entries = vec![Raw::Whole(big.clone())];
    for i in 0..16u8 {
        entries.push(Raw::RefDelta(big.id(), append_delta(big.data.len(), &[i])));
    }
    let pack = pack_of(&entries);
    assert!(pack.len() < 2 * 1024, "{} bytes", pack.len());
    let parsed = ParsedPack::parse_bounded(&pack, DEFAULT_MAX_OBJECT_BYTES, 8 * K).unwrap();
    let err = parsed
        .resolve(&HashMap::new(), DEFAULT_MAX_OBJECT_BYTES)
        .unwrap_err();
    assert!(err.to_string().contains("unpacks to over"), "{err}");
}

/// An entry that declares a size far past what its few compressed bytes
/// hold is refused as truncated (and is not given its declared size up
/// front).
#[test]
fn an_entry_declaring_a_huge_size_over_a_short_stream_is_refused() {
    let mut out = b"PACK".to_vec();
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend(size_header(3, 200 << 20));
    out.extend(deflate_zlib(b"short"));
    let sum = ObjectId::sha1(&out);
    out.extend_from_slice(sum.as_bytes());
    let err = ParsedPack::parse(&out, DEFAULT_MAX_OBJECT_BYTES).unwrap_err();
    assert!(
        err.to_string().contains("short of its declared size"),
        "{err}"
    );
}

// ── Receive-pack ────────────────────────────────────────────────────────────

const R: &str = "r";

async fn empty_repo() -> (MemoryHost, MemoryRepo) {
    use clax_git::RepoHost;
    let host = MemoryHost::new();
    host.create(R).await.unwrap();
    let repo = host.repo(R).unwrap();
    (host, repo)
}

fn commit(tree: ObjectId, parents: Vec<ObjectId>, message: &str) -> Object {
    let sig = Signature {
        name: "R".into(),
        email: "r@x".into(),
        time: 1,
        tz_offset_minutes: 0,
    };
    Commit {
        tree,
        parents,
        author: sig.clone(),
        committer: sig,
        message: message.into(),
    }
    .to_object()
    .unwrap()
}

fn tree(entries: &[(&str, Mode, ObjectId)]) -> Object {
    let entries: Vec<TreeEntry> = entries
        .iter()
        .map(|(n, m, id)| TreeEntry {
            mode: *m,
            name: n.to_string(),
            id: *id,
        })
        .collect();
    Object::new(ObjectKind::Tree, encode_tree(&entries).unwrap())
}

fn push_body(updates: &[RefUpdate], pack: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, u) in updates.iter().enumerate() {
        let caps = if i == 0 { "\0report-status" } else { "" };
        pktline::write(
            &mut out,
            format!("{} {} {}{caps}\n", u.old, u.new, u.name).as_bytes(),
        );
    }
    out.extend_from_slice(FLUSH);
    out.extend_from_slice(pack);
    out
}

fn report(body: &[u8]) -> Vec<String> {
    pktline::read_all(body)
        .unwrap()
        .iter()
        .filter_map(|p| p.text())
        .collect()
}

async fn clone_pack(repo: &MemoryRepo, want: ObjectId) -> Vec<u8> {
    let mut req = Vec::new();
    pktline::write(&mut req, format!("want {want}\n").as_bytes());
    req.extend_from_slice(FLUSH);
    pktline::write(&mut req, b"done\n");
    upload_pack(repo, &req).await
}

/// A pushed commit whose `tree` names a blob, and a tree whose
/// directory entry names a blob, are refused: accepted, `main` could
/// never be cloned again, and could not be rewound past them either.
#[tokio::test]
async fn a_push_naming_an_object_of_the_wrong_type_is_refused() {
    let (_host, repo) = empty_repo().await;
    let blob = Object::blob(b"not a tree".to_vec());
    let bad_commit = commit(blob.id(), vec![], "tree is a blob\n");
    let body = push_body(
        &[RefUpdate::new(MAIN, None, Some(bad_commit.id()))],
        &pack_of(&[Raw::Whole(blob.clone()), Raw::Whole(bad_commit.clone())]),
    );
    let out = receive_pack(&repo, &body, &ReceiveOptions::default()).await;
    let lines = report(&out.body);
    assert_eq!(lines[0], "unpack ok");
    assert!(lines[1].starts_with("ng refs/heads/main "), "{lines:?}");
    assert!(out.updated.is_empty());
    assert!(repo.refs().await.unwrap().is_empty());

    let dir_is_blob = tree(&[("d", Mode::TREE, blob.id())]);
    let c = commit(dir_is_blob.id(), vec![], "directory is a blob\n");
    let body = push_body(
        &[RefUpdate::new(MAIN, None, Some(c.id()))],
        &pack_of(&[
            Raw::Whole(blob.clone()),
            Raw::Whole(dir_is_blob),
            Raw::Whole(c),
        ]),
    );
    let out = receive_pack(&repo, &body, &ReceiveOptions::default()).await;
    let lines = report(&out.body);
    assert!(lines[1].starts_with("ng refs/heads/main "), "{lines:?}");

    // A well-typed push of the same blob is taken and clones.
    let good_tree = tree(&[("f", Mode::FILE, blob.id())]);
    let good = commit(good_tree.id(), vec![], "fine\n");
    let body = push_body(
        &[RefUpdate::new(MAIN, None, Some(good.id()))],
        &pack_of(&[
            Raw::Whole(blob),
            Raw::Whole(good_tree),
            Raw::Whole(good.clone()),
        ]),
    );
    let out = receive_pack(&repo, &body, &ReceiveOptions::default()).await;
    assert_eq!(report(&out.body)[1], "ok refs/heads/main");
    let pack = clone_pack(&repo, good.id()).await;
    assert!(pack.windows(4).any(|w| w == b"PACK"), "{:?}", report(&pack));
}

/// A thin pack whose second delta's base is the object its first delta
/// makes from a base already in the repository.
#[tokio::test]
async fn a_thin_pack_chaining_a_delta_on_another_deltas_result_is_taken() {
    let (_host, repo) = empty_repo().await;
    let v1 = Object::blob(b"line one\n".to_vec());
    let t1 = tree(&[("f", Mode::FILE, v1.id())]);
    let c1 = commit(t1.id(), vec![], "one\n");
    let st = repo
        .apply(
            vec![v1.clone(), t1, c1.clone()],
            &[RefUpdate::new(MAIN, None, Some(c1.id()))],
            false,
        )
        .await
        .unwrap();
    assert_eq!(st, [Ok(())]);

    let v2 = Object::blob(b"line one\nline two\n".to_vec());
    let v3 = Object::blob(b"line one\nline two\nline three\n".to_vec());
    let t3 = tree(&[("f", Mode::FILE, v3.id()), ("g", Mode::FILE, v2.id())]);
    let c3 = commit(t3.id(), vec![c1.id()], "three\n");
    let pack = pack_of(&[
        Raw::RefDelta(v1.id(), append_delta(v1.data.len(), b"line two\n")),
        Raw::RefDelta(v2.id(), append_delta(v2.data.len(), b"line three\n")),
        Raw::Whole(t3),
        Raw::Whole(c3.clone()),
    ]);
    let body = push_body(&[RefUpdate::new(MAIN, Some(c1.id()), Some(c3.id()))], &pack);
    let out = receive_pack(&repo, &body, &ReceiveOptions::default()).await;
    assert_eq!(
        report(&out.body),
        ["unpack ok", "ok refs/heads/main"],
        "{:?}",
        report(&out.body)
    );
    assert_eq!(repo.read_object(&v3.id()).await.unwrap(), Some(v3));
}

/// A push whose pack unpacks past `max_unpacked_bytes` is refused at
/// unpacking.
#[tokio::test]
async fn a_push_unpacking_past_the_limit_is_refused_at_unpack() {
    let (_host, repo) = empty_repo().await;
    let big = Object::blob(vec![b'x'; 64 * 1024]);
    let mut entries = vec![Raw::Whole(big.clone())];
    for i in 0..8u8 {
        entries.push(Raw::RefDelta(big.id(), append_delta(big.data.len(), &[i])));
    }
    let t = tree(&[("f", Mode::FILE, big.id())]);
    let c = commit(t.id(), vec![], "big\n");
    entries.push(Raw::Whole(t));
    entries.push(Raw::Whole(c.clone()));
    let body = push_body(
        &[RefUpdate::new(MAIN, None, Some(c.id()))],
        &pack_of(&entries),
    );
    let options = ReceiveOptions {
        max_unpacked_bytes: 4 * 64 * 1024,
        ..ReceiveOptions::default()
    };
    let out = receive_pack(&repo, &body, &options).await;
    let lines = report(&out.body);
    assert!(lines[0].contains("unpacks to over"), "{lines:?}");
    assert!(repo.refs().await.unwrap().is_empty());
}

// ── Upload-pack ─────────────────────────────────────────────────────────────

/// A stateless client sends its wants again in every request; when a
/// publish moves `main` between two of them, the old tip is no longer a
/// tip but is still reachable, and git's upload-pack serves it.
#[tokio::test]
async fn a_want_reachable_from_a_ref_is_served_after_the_ref_moved() {
    let (_host, repo) = empty_repo().await;
    let b1 = Object::blob(b"1".to_vec());
    let t1 = tree(&[("f", Mode::FILE, b1.id())]);
    let c1 = commit(t1.id(), vec![], "one\n");
    let b2 = Object::blob(b"2".to_vec());
    let t2 = tree(&[("f", Mode::FILE, b2.id())]);
    let c2 = commit(t2.id(), vec![c1.id()], "two\n");
    let dangling_tree = tree(&[("f", Mode::FILE, b2.id())]);
    let dangling = commit(dangling_tree.id(), vec![], "dangling\n");
    repo.apply(
        vec![b1, t1, c1.clone(), b2, t2, c2.clone(), dangling.clone()],
        &[RefUpdate::new(MAIN, None, Some(c2.id()))],
        false,
    )
    .await
    .unwrap();
    let pack = clone_pack(&repo, c1.id()).await;
    assert!(
        pack.windows(4).any(|w| w == b"PACK"),
        "{:?}",
        String::from_utf8_lossy(&pack)
    );
    let refused = clone_pack(&repo, dangling.id()).await;
    assert_eq!(
        report(&refused),
        [format!("ERR upload-pack: not our ref {}", dangling.id())]
    );
}

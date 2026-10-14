//! The rules every [`ContentStore`] keeps, as checks any implementation can
//! run: `content::contract::run(&store).await` panics on the first rule it
//! breaks. Each check uses artifacts of its own, so they share one store.
//! Built for this crate's tests, and for other crates with the
//! `content-contract` feature.

use super::{AssetKey, ContentStore, NewFile, VersionWrite};
use crate::ArtifactId;

/// Runs every check against `c`.
pub async fn run(c: &dyn ContentStore) {
    absent_content_reads_as_none(c).await;
    a_version_reads_back_file_by_file(c).await;
    carried_files_keep_their_bytes_and_versions_never_change(c).await;
    a_present_version_is_never_replaced(c).await;
    a_failed_write_leaves_nothing(c).await;
    an_unrecorded_version_can_be_removed_and_written_again(c).await;
    a_copy_holds_the_source_files(c).await;
    deleting_an_artifact_removes_only_its_content(c).await;
    assets_are_stored_read_and_deleted(c).await;
}

fn file<'a>(path: &'a str, bytes: &'a [u8]) -> NewFile<'a> {
    NewFile {
        path,
        bytes,
        content_type: "application/octet-stream",
    }
}

fn puts<'a>(files: &[(&'a str, &'a [u8])]) -> VersionWrite<'a> {
    VersionWrite {
        carry: Vec::new(),
        put: files.iter().map(|(p, b)| file(p, b)).collect(),
    }
}

fn asset(id: &ArtifactId, ext: &str) -> AssetKey {
    AssetKey {
        artifact: id.clone(),
        id: crate::new_ulid(),
        ext: ext.into(),
    }
}

async fn read(c: &dyn ContentStore, id: &ArtifactId, n: u32, path: &str) -> Option<Vec<u8>> {
    c.read_file(id, n, path).await.expect("read_file")
}

pub async fn absent_content_reads_as_none(c: &dyn ContentStore) {
    let id = ArtifactId::generate();
    assert_eq!(read(c, &id, 1, "index.html").await, None);
    c.write_version(&id, 1, &puts(&[("index.html", b"<p>1")]))
        .await
        .unwrap();
    assert_eq!(read(c, &id, 1, "missing.js").await, None);
    assert_eq!(read(c, &id, 1, "index.html/x").await, None);
    assert_eq!(read(c, &id, 2, "index.html").await, None);
    assert_eq!(c.read_asset(&asset(&id, "png")).await.unwrap(), None);
}

pub async fn a_version_reads_back_file_by_file(c: &dyn ContentStore) {
    let id = ArtifactId::generate();
    let binary: Vec<u8> = (0..=255).collect();
    c.write_version(
        &id,
        1,
        &puts(&[
            ("index.html", b"<p>1"),
            ("a/b/deep.js", b"deep()"),
            ("empty.txt", b""),
            ("bin.dat", &binary),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(read(c, &id, 1, "index.html").await.unwrap(), b"<p>1");
    assert_eq!(read(c, &id, 1, "a/b/deep.js").await.unwrap(), b"deep()");
    assert_eq!(read(c, &id, 1, "empty.txt").await.unwrap(), b"");
    assert_eq!(read(c, &id, 1, "bin.dat").await.unwrap(), binary);
}

pub async fn carried_files_keep_their_bytes_and_versions_never_change(c: &dyn ContentStore) {
    let id = ArtifactId::generate();
    c.write_version(
        &id,
        1,
        &puts(&[
            ("index.html", b"<p>1"),
            ("app.js", b"one"),
            ("gone.css", b"x"),
        ]),
    )
    .await
    .unwrap();
    let w = VersionWrite {
        carry: vec!["app.js"],
        put: vec![file("index.html", b"<p>2"), file("new.js", b"two")],
    };
    c.write_version(&id, 2, &w).await.unwrap();
    assert_eq!(read(c, &id, 2, "index.html").await.unwrap(), b"<p>2");
    assert_eq!(read(c, &id, 2, "app.js").await.unwrap(), b"one");
    assert_eq!(read(c, &id, 2, "new.js").await.unwrap(), b"two");
    assert_eq!(read(c, &id, 2, "gone.css").await, None, "not carried");
    // Version 1 is as it was written.
    assert_eq!(read(c, &id, 1, "index.html").await.unwrap(), b"<p>1");
    assert_eq!(read(c, &id, 1, "gone.css").await.unwrap(), b"x");
    assert_eq!(read(c, &id, 1, "new.js").await, None);
}

pub async fn a_present_version_is_never_replaced(c: &dyn ContentStore) {
    let id = ArtifactId::generate();
    c.write_version(&id, 1, &puts(&[("index.html", b"first")]))
        .await
        .unwrap();
    let again = c
        .write_version(&id, 1, &puts(&[("index.html", b"second"), ("x.js", b"x")]))
        .await;
    assert!(again.is_err(), "a second write of version 1 succeeded");
    assert_eq!(read(c, &id, 1, "index.html").await.unwrap(), b"first");
    assert_eq!(read(c, &id, 1, "x.js").await, None);
    let to = ArtifactId::generate();
    c.write_version(&to, 1, &puts(&[("index.html", b"kept")]))
        .await
        .unwrap();
    assert!(
        c.copy_version(&id, 1, &["index.html"], &to, 1)
            .await
            .is_err(),
        "a copy replaced a present version"
    );
    assert_eq!(read(c, &to, 1, "index.html").await.unwrap(), b"kept");
}

pub async fn a_failed_write_leaves_nothing(c: &dyn ContentStore) {
    let id = ArtifactId::generate();
    c.write_version(&id, 1, &puts(&[("index.html", b"<p>1")]))
        .await
        .unwrap();
    let w = VersionWrite {
        carry: vec!["not-there.js"],
        put: vec![file("index.html", b"<p>2")],
    };
    assert!(c.write_version(&id, 2, &w).await.is_err());
    assert_eq!(read(c, &id, 2, "index.html").await, None);
    let to = ArtifactId::generate();
    assert!(
        c.copy_version(&id, 1, &["index.html", "not-there.js"], &to, 1)
            .await
            .is_err()
    );
    assert_eq!(read(c, &to, 1, "index.html").await, None);
    // Nothing left behind blocks the next write of either version.
    c.write_version(&id, 2, &puts(&[("index.html", b"<p>2")]))
        .await
        .unwrap();
    c.copy_version(&id, 1, &["index.html"], &to, 1)
        .await
        .unwrap();
    assert_eq!(read(c, &id, 2, "index.html").await.unwrap(), b"<p>2");
    assert_eq!(read(c, &to, 1, "index.html").await.unwrap(), b"<p>1");
}

pub async fn an_unrecorded_version_can_be_removed_and_written_again(c: &dyn ContentStore) {
    let id = ArtifactId::generate();
    c.write_version(&id, 1, &puts(&[("index.html", b"<p>1")]))
        .await
        .unwrap();
    c.write_version(&id, 2, &puts(&[("index.html", b"lost"), ("f.js", b"f")]))
        .await
        .unwrap();
    c.remove_version(&id, 2).await.unwrap();
    assert_eq!(read(c, &id, 2, "index.html").await, None);
    assert_eq!(read(c, &id, 2, "f.js").await, None);
    c.remove_version(&id, 2).await.unwrap();
    c.remove_version(&ArtifactId::generate(), 1).await.unwrap();
    assert_eq!(read(c, &id, 1, "index.html").await.unwrap(), b"<p>1");
    let w = VersionWrite {
        carry: vec!["index.html"],
        put: Vec::new(),
    };
    c.write_version(&id, 2, &w).await.unwrap();
    assert_eq!(read(c, &id, 2, "index.html").await.unwrap(), b"<p>1");
    assert_eq!(read(c, &id, 2, "f.js").await, None);
}

pub async fn a_copy_holds_the_source_files(c: &dyn ContentStore) {
    let from = ArtifactId::generate();
    let to = ArtifactId::generate();
    c.write_version(
        &from,
        1,
        &puts(&[
            ("index.html", b"<p>src"),
            ("d/a.js", b"a"),
            ("left.css", b"l"),
        ]),
    )
    .await
    .unwrap();
    c.write_version(&to, 1, &puts(&[("index.html", b"<p>own")]))
        .await
        .unwrap();
    c.copy_version(&from, 1, &["index.html", "d/a.js"], &to, 2)
        .await
        .unwrap();
    assert_eq!(read(c, &to, 2, "index.html").await.unwrap(), b"<p>src");
    assert_eq!(read(c, &to, 2, "d/a.js").await.unwrap(), b"a");
    assert_eq!(read(c, &to, 2, "left.css").await, None, "not named");
    assert_eq!(read(c, &to, 1, "index.html").await.unwrap(), b"<p>own");
    assert_eq!(read(c, &from, 1, "left.css").await.unwrap(), b"l");
    // Copying within one artifact works the same way.
    c.copy_version(&to, 1, &["index.html"], &to, 3)
        .await
        .unwrap();
    assert_eq!(read(c, &to, 3, "index.html").await.unwrap(), b"<p>own");
}

pub async fn deleting_an_artifact_removes_only_its_content(c: &dyn ContentStore) {
    let gone = ArtifactId::generate();
    let kept = ArtifactId::generate();
    for id in [&gone, &kept] {
        c.write_version(id, 1, &puts(&[("index.html", b"<p>1"), ("a.js", b"a")]))
            .await
            .unwrap();
        c.write_version(id, 2, &puts(&[("index.html", b"<p>2")]))
            .await
            .unwrap();
    }
    let gone_asset = asset(&gone, "png");
    let kept_asset = asset(&kept, "png");
    c.put_asset(&gone_asset, b"g").await.unwrap();
    c.put_asset(&kept_asset, b"k").await.unwrap();
    c.delete_artifact(&gone).await.unwrap();
    for n in [1, 2] {
        assert_eq!(read(c, &gone, n, "index.html").await, None);
    }
    assert_eq!(read(c, &gone, 1, "a.js").await, None);
    assert_eq!(c.read_asset(&gone_asset).await.unwrap(), None);
    assert_eq!(read(c, &kept, 1, "a.js").await.unwrap(), b"a");
    assert_eq!(read(c, &kept, 2, "index.html").await.unwrap(), b"<p>2");
    assert_eq!(c.read_asset(&kept_asset).await.unwrap().unwrap(), b"k");
    c.delete_artifact(&gone).await.unwrap();
    c.delete_artifact(&ArtifactId::generate()).await.unwrap();
}

pub async fn assets_are_stored_read_and_deleted(c: &dyn ContentStore) {
    let id = ArtifactId::generate();
    let png = asset(&id, "png");
    let other = asset(&id, "woff2");
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    c.put_asset(&png, &big).await.unwrap();
    c.put_asset(&other, b"").await.unwrap();
    assert_eq!(c.read_asset(&png).await.unwrap().unwrap(), big);
    assert_eq!(c.read_asset(&other).await.unwrap().unwrap(), b"");
    let same_id_other_ext = AssetKey {
        ext: "jpg".into(),
        ..png.clone()
    };
    assert_eq!(c.read_asset(&same_id_other_ext).await.unwrap(), None);
    c.delete_asset(&png).await.unwrap();
    assert_eq!(c.read_asset(&png).await.unwrap(), None);
    assert_eq!(c.read_asset(&other).await.unwrap().unwrap(), b"");
    c.delete_asset(&png).await.unwrap();
    // Assets are not part of any version.
    c.write_version(&id, 1, &puts(&[("index.html", b"<p>1")]))
        .await
        .unwrap();
    assert_eq!(read(c, &id, 1, &format!("{}.woff2", other.id)).await, None);
}

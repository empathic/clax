//! Assets in a bucket: one object per asset under its artifact's prefix,
//! keys that never collide, and an artifact's deletion that pages through
//! the listing and leaves other artifacts' assets.

use clax_content_git::AssetStore;
use clax_core::ArtifactId;
use clax_core::content::{AssetKey, wait};

use crate::repos::assets::{BucketAssets, DELETE_BATCH, MemObjects, object_key};

fn key(artifact: &ArtifactId, id: &str, ext: &str) -> AssetKey {
    AssetKey {
        artifact: artifact.clone(),
        id: id.into(),
        ext: ext.into(),
    }
}

#[test]
fn an_asset_is_one_object_under_its_artifact_and_keys_never_collide() {
    let a = ArtifactId::parse("abcdefghjkmn").unwrap();
    assert_eq!(
        object_key(&key(&a, "logo", "png")),
        "assets/abcdefghjkmn/logo.png"
    );
    // A dot or slash in an ID or extension is encoded, so these differ.
    let keys = [
        object_key(&key(&a, "a.b", "c")),
        object_key(&key(&a, "a", "b.c")),
        object_key(&key(&a, "a/../b", "c")),
        object_key(&key(&a, "a%2Eb", "c")),
    ];
    for (i, k) in keys.iter().enumerate() {
        assert!(k.starts_with("assets/abcdefghjkmn/"), "{k}");
        assert_eq!(k.matches('/').count(), 2, "{k}");
        for other in &keys[i + 1..] {
            assert_ne!(k, other);
        }
    }
}

#[test]
fn put_replaces_read_answers_none_once_deleted() {
    let s = BucketAssets {
        objects: MemObjects::new(10),
    };
    let a = ArtifactId::generate();
    let k = key(&a, "x", "txt");
    wait(async {
        assert_eq!(s.read_asset(&k).await.unwrap(), None);
        s.put_asset(&k, b"one").await.unwrap();
        s.put_asset(&k, b"two").await.unwrap();
        assert_eq!(
            s.read_asset(&k).await.unwrap().as_deref(),
            Some(&b"two"[..])
        );
        s.delete_asset(&k).await.unwrap();
        s.delete_asset(&k).await.unwrap();
        assert_eq!(s.read_asset(&k).await.unwrap(), None);
    });
}

#[test]
fn deleting_an_artifacts_assets_pages_through_them_and_keeps_the_others() {
    let s = BucketAssets {
        objects: MemObjects::new(7),
    };
    let a = ArtifactId::generate();
    let b = ArtifactId::generate();
    let n = DELETE_BATCH + 5;
    wait(async {
        for i in 0..n {
            s.put_asset(&key(&a, &format!("{i}"), "bin"), b"a")
                .await
                .unwrap();
        }
        s.put_asset(&key(&b, "keep", "bin"), b"b").await.unwrap();
        s.delete_assets(&a).await.unwrap();
        assert_eq!(s.objects.keys(), vec![object_key(&key(&b, "keep", "bin"))]);
        // None left is no error.
        s.delete_assets(&a).await.unwrap();
    });
}

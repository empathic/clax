//! The bundles apps published from the Clax app carry on a hosted Clax,
//! and the installs and forks it counts of them.

use clax_core::audit::AuditCtx;
use clax_core::publish::{PublishRequest, validate};
use clax_core::{ArtifactId, CoreError, Home};
use clax_store::Store;
use clax_store::app_bundles::MAX_BUNDLE_BYTES;

const DAEMON: &AuditCtx = &AuditCtx::DAEMON;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
    (dir, st)
}

fn page(body: &str, if_version: Option<u32>) -> clax_core::publish::ValidatedPublish {
    let req: PublishRequest = serde_json::from_value(serde_json::json!({
        "title": "Tide Clock",
        "if_version": if_version,
        "files": {"index.html": {"content": body, "encoding": "utf8"}}
    }))
    .unwrap();
    validate(req).unwrap()
}

/// A new app and its first version's `content_sha256`.
fn app(st: &Store) -> (ArtifactId, String) {
    let (a, v) = st
        .create_artifact(DAEMON, page("<p>one</p>", None), None)
        .unwrap();
    (ArtifactId::parse(&a.id).unwrap(), v.content_sha256.unwrap())
}

/// The number and `content_sha256` of the version `body` publishes.
fn publish(st: &Store, aid: &ArtifactId, body: &str) -> (u32, String) {
    let at = st.get_artifact(aid).unwrap().unwrap().current_version;
    let (_, v) = st
        .publish_version(DAEMON, aid, page(body, Some(at)), None)
        .unwrap();
    (v.n, v.content_sha256.unwrap())
}

fn code<T: std::fmt::Debug>(r: clax_core::Result<T>) -> String {
    match r.unwrap_err() {
        CoreError::Invalid { code, .. } => code.to_string(),
        e => format!("{e:?}"),
    }
}

fn rows(st: &Store, aid: &ArtifactId) -> Vec<u32> {
    (1..=5)
        .filter(|n| st.app_bundle(aid, *n).unwrap().is_some())
        .collect()
}

#[test]
fn put_app_bundle_refuses_mismatch_lineage_order_and_size() {
    let (_d, st) = store();
    let (aid, c1) = app(&st);
    let b1 = st
        .put_app_bundle(
            DAEMON,
            &aid,
            1,
            "lin-a",
            7,
            &c1,
            b"bundle one",
            Some("p_ed"),
        )
        .unwrap();
    assert_eq!(
        (b1.version, b1.bundle_version, b1.size, b1.person.as_deref()),
        (1, 7, 10, Some("p_ed"))
    );
    assert_eq!(b1.content_sha256, c1);
    assert_eq!(b1.sha256, clax_core::secret::sha256_hex("bundle one"));
    let (n2, c2) = publish(&st, &aid, "<p>two</p>");

    // The version's files are not the bundle's.
    assert_eq!(
        code(st.put_app_bundle(DAEMON, &aid, n2, "lin-a", 8, &c1, b"two", None)),
        "bundle_mismatch"
    );
    // Another lineage than the artifact's first bundle's.
    assert_eq!(
        code(st.put_app_bundle(DAEMON, &aid, n2, "lin-b", 8, &c2, b"two", None)),
        "bundle_lineage"
    );
    // Not above the last bundle's release counter, equal or lower.
    for bv in [7, 3] {
        assert_eq!(
            code(st.put_app_bundle(DAEMON, &aid, n2, "lin-a", bv, &c2, b"two", None)),
            "bundle_version_order",
            "{bv}"
        );
    }
    // Over the cap by one byte; at the cap is taken.
    let over = vec![0u8; MAX_BUNDLE_BYTES as usize + 1];
    assert_eq!(
        code(st.put_app_bundle(DAEMON, &aid, n2, "lin-a", 8, &c2, &over, None)),
        "bundle_too_large"
    );
    assert_eq!(rows(&st, &aid), [1], "no refusal wrote a row");
    let at_cap = vec![1u8; MAX_BUNDLE_BYTES as usize];
    let b2 = st
        .put_app_bundle(DAEMON, &aid, n2, "lin-a", 8, &c2, &at_cap, None)
        .unwrap();
    assert_eq!(b2.size, MAX_BUNDLE_BYTES);
    // A number may skip.
    let (n3, c3) = publish(&st, &aid, "<p>three</p>");
    st.put_app_bundle(DAEMON, &aid, n3, "lin-a", 20, &c3, b"three", None)
        .unwrap();
    assert_eq!(rows(&st, &aid), [1, 2, 3]);
    assert_eq!(st.latest_app_bundle(&aid).unwrap().unwrap().version, n3);
    assert_eq!(st.read_app_bundle(&b1).unwrap(), b"bundle one");
    assert_eq!(st.read_app_bundle(&b2).unwrap(), at_cap);
    // A version that is not there carries no bundle.
    assert!(matches!(
        st.put_app_bundle(DAEMON, &aid, 9, "lin-a", 30, &c3, b"nine", None),
        Err(CoreError::NotFound)
    ));
}

#[test]
fn put_app_bundle_same_bytes_twice_is_one_row() {
    let (_d, st) = store();
    let (aid, c1) = app(&st);
    assert!(st.latest_app_bundle(&aid).unwrap().is_none());
    let first = st
        .put_app_bundle(DAEMON, &aid, 1, "lin", 1, &c1, b"same", None)
        .unwrap();
    // A retry whose first answer was lost: the same row, not a refusal of
    // its release counter.
    let again = st
        .put_app_bundle(DAEMON, &aid, 1, "lin", 1, &c1, b"same", None)
        .unwrap();
    assert_eq!(again, first);
    assert_eq!(st.app_bundle(&aid, 1).unwrap(), Some(first.clone()));
    assert_eq!(st.latest_app_bundle(&aid).unwrap(), Some(first.clone()));
    assert_eq!(st.read_app_bundle(&first).unwrap(), b"same");
    // Other bytes for a version that has its bundle are refused.
    assert_eq!(
        code(st.put_app_bundle(DAEMON, &aid, 1, "lin", 1, &c1, b"other", None)),
        "bundle_mismatch"
    );
    assert_eq!(st.read_app_bundle(&first).unwrap(), b"same");
}

#[test]
fn install_count_counts_distinct_installs() {
    let (_d, st) = store();
    let (aid, _) = app(&st);
    let (other, _) = app(&st);
    assert_eq!(st.install_count(&aid).unwrap(), 0);
    st.record_install(&aid, "dev-1").unwrap();
    st.record_install(&aid, "dev-1").unwrap();
    st.record_install(&aid, "dev-2").unwrap();
    st.record_install(&other, "dev-1").unwrap();
    assert_eq!(st.install_count(&aid).unwrap(), 2);
    assert_eq!(st.install_count(&other).unwrap(), 1);
}

#[test]
fn fork_count_counts_forks() {
    let (_d, st) = store();
    let (aid, _) = app(&st);
    let (other, _) = app(&st);
    assert_eq!(st.fork_count(&aid).unwrap(), 0);
    let (f1, _) = st.fork_artifact(DAEMON, &aid, None, None, None).unwrap();
    st.fork_artifact(DAEMON, &aid, None, None, None).unwrap();
    st.fork_artifact(DAEMON, &other, None, None, None).unwrap();
    assert_eq!(st.fork_count(&aid).unwrap(), 2);
    // A deleted fork is not counted.
    st.delete_artifact(DAEMON, &ArtifactId::parse(&f1.id).unwrap())
        .unwrap();
    assert_eq!(st.fork_count(&aid).unwrap(), 1);
    assert_eq!(st.fork_count(&other).unwrap(), 1);
}

#[test]
fn a_bundle_version_over_what_the_database_keeps_is_refused() {
    let (_d, st) = store();
    let (aid, c1) = app(&st);
    let over = i64::MAX as u64 + 1;
    assert_eq!(
        code(st.put_app_bundle(DAEMON, &aid, 1, "lin", over, &c1, b"one", None)),
        "invalid_args"
    );
    assert!(st.app_bundle(&aid, 1).unwrap().is_none());
    let top = st
        .put_app_bundle(DAEMON, &aid, 1, "lin", i64::MAX as u64, &c1, b"one", None)
        .unwrap();
    assert_eq!(top.bundle_version, i64::MAX as u64);
    assert_eq!(st.app_bundle(&aid, 1).unwrap(), Some(top));
    // Nothing is above it, so the counter never goes back.
    let (n2, c2) = publish(&st, &aid, "<p>two</p>");
    assert_eq!(
        code(st.put_app_bundle(DAEMON, &aid, n2, "lin", 1, &c2, b"two", None)),
        "bundle_version_order"
    );
}

#[test]
fn a_deleted_app_takes_no_bundle_or_install_and_its_bundles_go_with_it() {
    let (_d, st) = store();
    let (aid, c1) = app(&st);
    let b = st
        .put_app_bundle(DAEMON, &aid, 1, "lin", 1, &c1, b"one", None)
        .unwrap();
    st.record_install(&aid, "dev-1").unwrap();
    st.delete_artifact(DAEMON, &aid).unwrap();
    assert!(st.app_bundle(&aid, 1).unwrap().is_none());
    assert!(st.latest_app_bundle(&aid).unwrap().is_none());
    assert_eq!(st.install_count(&aid).unwrap(), 0);
    assert!(matches!(st.read_app_bundle(&b), Err(CoreError::NotFound)));
    assert!(matches!(
        st.put_app_bundle(DAEMON, &aid, 1, "lin", 2, &c1, b"one", None),
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        st.record_install(&aid, "dev-2"),
        Err(CoreError::NotFound)
    ));
    assert_eq!(st.install_count(&aid).unwrap(), 0);
}

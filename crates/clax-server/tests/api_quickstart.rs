use clax_testkit::TestServer;

/// The quickstart page ships in the web build for the gallery to seed, never
/// to be served: the shell's origin answers neither its own path nor a
/// traversal into it with the page.
#[tokio::test]
async fn the_shell_origin_serves_no_quickstart_file() {
    let ts = TestServer::spawn().await;
    for path in ["/_clax/../quickstart/index.html", "/quickstart/index.html"] {
        let res = ts.get(path).await;
        let status = res.status();
        let body = res.text().await.unwrap();
        assert!(
            !body.contains("Your agents build pages"),
            "{path} answered {status} with the quickstart page"
        );
    }
}

/// The page the web build ships seeds a gallery: it meets every publish
/// rule, and its version 1 holds every shipped file. Under Bazel the web
/// build is this test's data, so the page is always there.
#[tokio::test]
async fn the_shipped_quickstart_seeds_a_gallery() {
    let ts = TestServer::spawn().await;
    let Some(release) = clax_core::quickstart::Release::from_tree(&*ts.web) else {
        assert!(
            std::env::var_os("TEST_SRCDIR").is_none(),
            "the web build under Bazel carries no quickstart/"
        );
        eprintln!("skipped: the web UI is not built");
        return;
    };
    let clax_store::Seeded::Made { artifact, version } = ts
        .store
        .seed_quickstart(&clax_core::audit::AuditCtx::DAEMON, None, &release)
        .await
        .unwrap()
    else {
        panic!("a new home's gallery is seeded");
    };
    assert_eq!(artifact.title, clax_core::quickstart::TITLE);
    assert!(artifact.pinned);
    let shipped: Vec<&String> = release.files.keys().collect();
    assert_eq!(version.files.keys().collect::<Vec<_>>(), shipped);
    assert!(
        shipped.iter().any(|p| p.as_str() == "icon.svg"),
        "{shipped:?}"
    );
}

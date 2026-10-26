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

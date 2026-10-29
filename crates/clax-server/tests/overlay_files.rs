//! The embeddable overlay's files (`/_clax/overlay.js`, `overlay.mjs`,
//! `overlay.d.ts`): served to any origin, without credentials, with their
//! JavaScript or type-definition content type, so an outside product can
//! load the script, `import()` the module and read its types from a Clax
//! instance. The test daemon serves a temporary web UI, never `web/dist`.

use clax_testkit::TestServer;

#[tokio::test]
async fn the_overlay_files_are_served_to_any_origin_with_their_types() {
    let dist = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dist.path().join("_clax")).unwrap();
    for (name, body) in [
        ("overlay.js", "var Clax=1;"),
        ("overlay.mjs", "var o=1;export{o as overlay};"),
        ("overlay.d.ts", "export declare function overlay(): void;"),
        ("mark.svg", "<svg/>"),
    ] {
        std::fs::write(dist.path().join("_clax").join(name), body).unwrap();
    }
    let ts = TestServer::spawn_with_web(dist.path()).await;
    let client = reqwest::Client::new();
    for (name, ty) in [
        ("overlay.js", "text/javascript"),
        ("overlay.mjs", "text/javascript"),
        ("overlay.d.ts", "text/plain; charset=utf-8"),
    ] {
        let res = client
            .get(format!("{}/_clax/{name}", ts.base))
            .header("origin", "https://changelist.example")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{name}");
        let h = res.headers();
        assert_eq!(h["content-type"], ty, "{name}");
        assert_eq!(h["access-control-allow-origin"], "*", "{name}");
        assert!(h.get("access-control-allow-credentials").is_none(), "{name}");
        assert_eq!(h["x-content-type-options"], "nosniff", "{name}");
    }
    // Any other file of the web UI is not offered to other origins.
    let res = client
        .get(format!("{}/_clax/mark.svg", ts.base))
        .header("origin", "https://changelist.example")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.headers().get("access-control-allow-origin").is_none());
}

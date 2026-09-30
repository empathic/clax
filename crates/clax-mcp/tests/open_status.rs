//! `open` reports whether the platform opener succeeded. Alone in its test
//! binary because it puts a fake opener first on `PATH`.

use clax_mcp::tools::{PublishArgs, TargetArgs};
use clax_mcp::{ClaxTools, DaemonClient};
use clax_server::testing::TestServer;
use rmcp::handler::server::wrapper::Parameters;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

fn text(r: &rmcp::model::CallToolResult) -> Value {
    serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn opened_follows_the_openers_exit_status() {
    // A fake `open` and `xdg-open` that behave as the file `mode` says: exit
    // with the code it holds, or keep running when it says `hang`.
    let dir = tempfile::tempdir().unwrap();
    let mode = dir.path().join("mode");
    let script = format!(
        "#!/bin/sh\nm=$(cat '{}')\nif [ \"$m\" = hang ]; then sleep 5; exit 0; fi\nexit \"$m\"\n",
        mode.display()
    );
    for name in ["open", "xdg-open"] {
        let p = dir.path().join(name);
        std::fs::write(&p, &script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // SAFETY: this is the only test in the binary and it sets the variables
    // before starting any thread that could read the environment.
    unsafe {
        std::env::set_var("PATH", path);
        std::env::remove_var("CLAX_NO_OPEN");
    }

    let ts = TestServer::spawn().await;
    let t = ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None),
        format!("http://localhost:{}", ts.addr.port()),
        None,
        ts.home.log_path(),
    );
    let published = t
        .publish(Parameters(PublishArgs {
            html: Some("<title>Open me</title>".into()),
            ..Default::default()
        }))
        .await
        .unwrap();
    let id = text(&published)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let open = || async {
        let r = t
            .open(Parameters(TargetArgs {
                url_or_id: id.clone(),
            }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false));
        text(&r)
    };

    std::fs::write(&mode, "1").unwrap();
    let o = open().await;
    assert_eq!(o["opened"], false, "{o}");
    assert_eq!(
        o["url"],
        format!("http://localhost:{}/a/{id}", ts.addr.port())
    );

    std::fs::write(&mode, "0").unwrap();
    assert_eq!(open().await["opened"], true);

    // An opener still running after the wait counts as opened.
    std::fs::write(&mode, "hang").unwrap();
    let t0 = Instant::now();
    assert_eq!(open().await["opened"], true);
    let took = t0.elapsed();
    assert!(
        took >= Duration::from_millis(1400) && took < Duration::from_secs(3),
        "{took:?}"
    );
}

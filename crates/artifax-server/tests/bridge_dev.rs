//! A debug build reads the bridge from `web/dist` on every request (as
//! `just dev` rebuilds it under a running daemon): a rebuilt bridge gets a new
//! URL in the next page served, and the bridge is never immutable. This test
//! rewrites the built bridge and restores it; it is its own test binary so no
//! other test reads the bridge meanwhile.
#![cfg(debug_assertions)]

mod common;
use artifax_server::routes::shell::bridge_version;
use common::TestServer;
use std::path::PathBuf;

/// Puts the bridge file back as it was (or removes it) when dropped.
struct Restore {
    path: PathBuf,
    before: Option<Vec<u8>>,
    made_dir: bool,
}

impl Drop for Restore {
    fn drop(&mut self) {
        match &self.before {
            Some(b) => std::fs::write(&self.path, b).unwrap(),
            None => {
                let _ = std::fs::remove_file(&self.path);
                if self.made_dir {
                    let _ = std::fs::remove_dir(self.path.parent().unwrap());
                }
            }
        }
    }
}

#[tokio::test]
async fn a_rebuilt_bridge_gets_a_new_url_and_is_never_immutable() {
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../web/dist/_artifax/bridge.js"
    ));
    let dir = path.parent().unwrap().to_path_buf();
    let made_dir = !dir.exists();
    std::fs::create_dir_all(&dir).unwrap();
    let _restore = Restore {
        before: std::fs::read(&path).ok(),
        path: path.clone(),
        made_dir,
    };
    std::fs::write(&path, "/* bridge A */").unwrap();

    let ts = TestServer::spawn().await;
    let created = ts.publish("R", &[("index.html", "<p>i</p>")]).await;
    let id = created["artifact"]["id"].as_str().unwrap();
    let page = |id: String| {
        let ts = &ts;
        async move { ts.get(&format!("/c/{id}/v/1/")).await.text().await.unwrap() }
    };
    let a = bridge_version();
    assert_eq!(a.len(), 12);
    assert!(
        page(id.to_string())
            .await
            .contains(&format!("bridge.js?v={a}\""))
    );
    let res = ts.get(&format!("/_artifax/bridge.js?v={a}")).await;
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert_eq!(res.text().await.unwrap(), "/* bridge A */");

    std::fs::write(&path, "/* bridge B, rebuilt */").unwrap();
    let b = bridge_version();
    assert_ne!(a, b, "the version follows the bytes on disk");
    let html = page(id.to_string()).await;
    assert!(html.contains(&format!("bridge.js?v={b}\"")), "{html}");
    assert!(!html.contains(&a), "{html}");
    let res = ts.get(&format!("/_artifax/bridge.js?v={b}")).await;
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert_eq!(res.text().await.unwrap(), "/* bridge B, rebuilt */");
}

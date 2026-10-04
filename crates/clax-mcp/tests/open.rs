//! `open` with the opener turned off.

use clax_mcp::tools::{Opener, PublishArgs, TargetArgs};
use clax_mcp::{ClaxTools, DaemonClient};
use clax_server::testing::TestServer;
use rmcp::handler::server::wrapper::Parameters;
use serde_json::Value;

fn text(r: &rmcp::model::CallToolResult) -> Value {
    serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap()
}

#[tokio::test]
async fn open_returns_the_browser_url_without_opening_when_the_opener_is_off() {
    let ts = TestServer::spawn().await;
    let t = ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None),
        format!("http://localhost:{}", ts.addr.port()),
        None,
        ts.home.log_path(),
    )
    .with_opener(Opener::Off);
    let published = t
        .publish(Parameters(PublishArgs {
            html: Some("<p>".into()),
            title: Some("Open me".into()),
            ..Default::default()
        }))
        .await
        .unwrap();
    let id = text(&published)["artifact_id"]
        .as_str()
        .unwrap()
        .to_string();

    let o = t
        .open(Parameters(TargetArgs {
            url_or_id: id.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(o.is_error, Some(false));
    let o = text(&o);
    assert_eq!(
        o["url"],
        format!("http://localhost:{}/a/{id}", ts.addr.port())
    );
    assert_eq!(o["opened"], false);
    assert_eq!(o["feedback"], serde_json::json!([]));

    let e = t
        .open(Parameters(TargetArgs {
            url_or_id: "not-an-id".into(),
        }))
        .await
        .unwrap();
    assert_eq!(e.is_error, Some(true));
    assert_eq!(text(&e)["error"]["code"], "invalid_id");
}

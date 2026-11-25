//! The tools a run of the fake's kind `verdict` (`src/fake.d.ts`) is
//! sent: `finish` takes the kind's result, `Verdict`, as JSON Schema,
//! and its description ends with that interface's doc comment.

use super::names as g;
use super::pins::pin_normalised;
use crate::finish::{Typed, VERDICT};
use serde_json::json;

#[tokio::test]
async fn a_run_of_a_hosts_kind_is_sent_its_kinds_result_as_finishs_schema() {
    let t = Typed::new().await;
    t.finishes(
        Some(VERDICT),
        &[json!({"title": "Fair", "status": "upheld"})],
    )
    .await;
    let reqs = t.env.api.requests();
    // As sent, the result's fields are in the definition's order (the
    // pin reads the body as a `Value`, whose maps sort them).
    let raw = &reqs[0].raw;
    // A tool's members are sorted: its schema comes before its name.
    let name = raw.find(r#""name":"finish""#).expect("the finish tool");
    let schema = raw[..name]
        .rfind(r#""input_schema":"#)
        .expect("the finish tool's schema");
    let sent = &raw[schema..name];
    let field = |name: &str| sent.find(&format!("\"{name}\":{{")).unwrap();
    assert!(
        field("title") < field("confidence") && field("confidence") < field("status"),
        "{sent}"
    );
    assert!(
        sent.starts_with(r#""input_schema":{"type":"object","properties":{"title":"#),
        "{sent}"
    );
    pin_normalised(
        "the tools of a run of fake.verdict",
        g::REQUEST_FAKE_VERDICT_TOOLS,
        &reqs[0].body["tools"],
    );
}

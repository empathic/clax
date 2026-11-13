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
    pin_normalised(
        "the tools of a run of fake.verdict",
        g::REQUEST_FAKE_VERDICT_TOOLS,
        &reqs[0].body["tools"],
    );
}

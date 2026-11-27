//! Formal source for the browser-side WebMCP tools' input schemas
//! (`web/shell/src/webmcp.ts`, registered via `document.modelContext`).
//!
//! Those tools run as page script, not through this crate's stdio `rmcp`
//! router, and their inputs are page-scoped (no `url_or_id`, no pagination),
//! so they do not reuse `tools::CommentsReadArgs` as-is: that struct carries
//! fields (`url_or_id`, `cursor`, `include_resolved`) a same-page tool has no
//! use for. Each gets its own small schemars-derived type instead, so the
//! `inputSchema` `webmcp.ts` registers still comes from one formal
//! (JSON Schema) source rather than a hand-written literal: this module's
//! test checks the committed `web/shell/src/webmcp.schemas.json` against
//! what these types generate, and `webmcp.schema.test.ts` checks the TS tool
//! registrations against that same committed file.
//!
//! Follow-up: `thread_id` here and in `tools::CommentsReadArgs` /
//! `tools::CommentsReplyArgs` / `tools::CommentsResolveArgs` are still
//! separate string fields on separate structs; a shared newtype would close
//! that gap, and let a page-scoped WebMCP tool and its stdio-MCP equivalent
//! share one argument type wherever their shape actually matches.

use schemars::JsonSchema;
use serde::Deserialize;

/// Input for `clax_artifact_info` and `clax_list_threads`: both read the
/// page's already-loaded state and take nothing.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebMcpEmptyArgs {}

/// Input for `clax_read_thread`.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebMcpReadThreadArgs {
    /// A thread ID from clax_list_threads.
    #[schemars(length(min = 1))]
    pub thread_id: String,
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use schemars::schema_for;
    use serde_json::Value;
    use std::path::Path;

    /// The object-schema core an MCP tool registers as `inputSchema`:
    /// `schema_for!`'s output, less the keys that describe the schema
    /// document rather than the shape of a call's arguments: `$schema` and
    /// `title` (schemars' own bookkeeping), and the top-level `description`
    /// (the struct's own doc comment, which exists for readers of this
    /// module, not for an agent calling the tool — that text belongs to the
    /// `Tool.description` each tool already carries in `webmcp.ts`, not
    /// duplicated into its `inputSchema`). A field's own `description`
    /// (`thread_id`'s, for instance) is kept: that one *is* part of the
    /// argument's shape. Every other key (`properties`, `required`,
    /// `additionalProperties`, `minLength`, …) is kept too, so a new
    /// constraint added to a Rust type is never silently dropped here and
    /// missed by the comparison below.
    fn tool_schema<T: JsonSchema>() -> Value {
        let mut v = serde_json::to_value(schema_for!(T)).expect("schema serializes");
        if let Value::Object(map) = &mut v {
            map.remove("$schema");
            map.remove("title");
            map.remove("description");
        }
        v
    }

    fn committed_schemas() -> Value {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/shell/src/webmcp.schemas.json");
        serde_json::from_slice(&std::fs::read(&path).expect("read webmcp.schemas.json"))
            .expect("webmcp.schemas.json is JSON")
    }

    /// `web/shell/src/webmcp.ts` imports `webmcp.schemas.json` directly for
    /// its tools' `inputSchema`, so there is nothing on the TS side to drift.
    /// This test is what keeps that committed file itself from drifting out
    /// of sync with the Rust types it was generated from: regenerate it
    /// (print `tool_schema::<T>()` for the failing tool and paste it in)
    /// whenever this test fails after an intentional change to a type above.
    #[test]
    fn checked_in_schemas_match_the_rust_source() {
        let committed = committed_schemas();
        assert_eq!(
            committed["clax_artifact_info"],
            tool_schema::<WebMcpEmptyArgs>(),
            "clax_artifact_info"
        );
        assert_eq!(
            committed["clax_list_threads"],
            tool_schema::<WebMcpEmptyArgs>(),
            "clax_list_threads"
        );
        assert_eq!(
            committed["clax_read_thread"],
            tool_schema::<WebMcpReadThreadArgs>(),
            "clax_read_thread"
        );
    }
}

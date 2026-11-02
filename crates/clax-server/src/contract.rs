//! The public operations of this server's routes, as the API contract
//! (`clax_core::contract`, spec 2026-10-27-clax-sdk-design §4.1) has them,
//! and the committed contract they generate: `web/sdk/openapi.json`.
//!
//! The reporter router (`routes::queue_reporter`) is public whole: its
//! tests fail for a route there without an operation. Elsewhere a route is
//! internal until its operation is listed in [`ops`]; each listed one is
//! checked against the route table (`router.rs`).
//!
//! `just contract` writes the file again after a change; the tests fail
//! while it differs from what the routes generate.

use crate::routes::{audit_trail, discovery, queue_agent, queue_reporter, queues};
use clax_core::contract::{self, Op};

/// The committed contract's path, from the repository's root.
pub const COMMITTED: &str = "web/sdk/openapi.json";

/// Every public operation, in the contract's order.
pub fn ops() -> Vec<&'static Op> {
    let mut ops = vec![
        &discovery::GET,
        &audit_trail::LIST,
        &audit_trail::GET,
        &queues::LIST,
        &queues::CREATE_GRANT,
    ];
    ops.extend(queue_reporter::CONTRACT.iter().copied());
    ops.extend([&queue_agent::CLAIM, &queue_agent::RENEW]);
    ops
}

/// The contract the routes generate, as [`COMMITTED`] holds it.
pub fn render() -> String {
    contract::render(&ops())
}

/// The `(path, method, handler)` of each method of each `.route(…)` in the
/// router source `src`: `.route("/p", get(a).post(b))` is `("/p", "get",
/// "a")` and `("/p", "post", "b")`. A handler is named as the source names
/// it (`queues::list`).
pub fn routed(src: &str) -> Vec<(String, &'static str, String)> {
    const METHODS: [&str; 6] = ["get", "post", "put", "patch", "delete", "options"];
    let mut out = Vec::new();
    for chunk in src.split(".route(").skip(1) {
        let Some(rest) = chunk.trim_start().strip_prefix('"') else {
            continue;
        };
        let Some(end) = rest.find('"') else { continue };
        let path = &rest[..end];
        // The route's own arguments: up to its closing parenthesis.
        let mut depth = 1usize;
        let mut close = rest.len();
        for (i, c) in rest.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let args = &rest[end + 1..close];
        let bytes = args.as_bytes();
        for m in METHODS {
            let pat = format!("{m}(");
            let mut from = 0;
            while let Some(at) = args[from..].find(&pat) {
                let at = from + at;
                from = at + pat.len();
                // A method call (`get(`, `.post(`), not part of a longer name.
                if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
                    continue;
                }
                let name = &args[from..];
                let name = &name[..name.find(')').unwrap_or(name.len())];
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
                {
                    out.push((path.to_string(), m, name.to_string()));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_contract_is_what_the_routes_generate() {
        let generated = render();
        if std::env::var_os("CLAX_CONTRACT_WRITE").is_some() {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(COMMITTED);
            std::fs::write(&path, &generated).unwrap();
            return;
        }
        let committed = include_str!("../../../web/sdk/openapi.json");
        assert!(
            committed == generated,
            "{COMMITTED} is not what the routes generate: run `just contract` and commit it"
        );
    }

    /// Each listed operation outside the reporter router is a route of the
    /// route table, by path, method and handler.
    #[test]
    fn each_operation_is_routed() {
        let table = routed(include_str!("router.rs"));
        for op in ops() {
            if queue_reporter::CONTRACT.iter().any(|o| o.id == op.id) {
                continue;
            }
            assert!(
                table
                    .iter()
                    .any(|(p, m, h)| p == op.path && *m == op.method.as_str() && h == op.handler),
                "{} {} {} is not in router.rs",
                op.method.as_str(),
                op.path,
                op.handler
            );
        }
    }

    #[test]
    fn routed_reads_a_route_table() {
        let src = r#"
            .route("/a/{x}", get(a::one).post(two).layer(L).options(preflight))
            .route(
                "/b",
                delete(three),
            )
            .route_layer(x)
        "#;
        assert_eq!(
            routed(src),
            vec![
                ("/a/{x}".into(), "get", "a::one".into()),
                ("/a/{x}".into(), "post", "two".into()),
                ("/a/{x}".into(), "options", "preflight".into()),
                ("/b".into(), "delete", "three".into()),
            ]
        );
    }

    /// Discovery serves the contract's major.
    #[test]
    fn discovery_serves_the_major() {
        assert_eq!(discovery::CONTRACT_VERSION, contract::MAJOR);
        let doc = contract::openapi(&ops());
        assert_eq!(doc["info"]["version"], contract::VERSION);
    }
}

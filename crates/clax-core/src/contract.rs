//! The API contract (spec 2026-10-27-clax-sdk-design D2, §4.1), generated
//! from the routes rather than written first: each public route declares an
//! [`Op`] beside its handler (method, path, request and response types,
//! error codes, and §4.1's `retry`, `channel`, `rights` and `stream`), and
//! [`openapi`] assembles the ops into an OpenAPI 3.1 document whose schemas
//! come from the same Rust types the handlers serialize (schemars).
//!
//! A route left out of the contract is internal. The document is committed
//! (`web/sdk/openapi.json`); clax-server's tests fail when the routes no
//! longer generate it, the SDK's TypeScript types are generated from it
//! (`web/scripts/contract-types.mjs`), and `scripts/check-contract.py`
//! fails a breaking change against the last release that does not raise
//! [`VERSION`]'s major.
//!
//! The `x-clax-*` extensions on each operation:
//! - `x-clax-retry`: whether a client may send it again after a network
//!   error that brought no answer (sending it twice does what sending it
//!   once does);
//! - `x-clax-channel`: the credential kinds it accepts ([`Channel`]);
//! - `x-clax-rights`: what the credential must hold: a
//!   `clax_core::rights` action, a queue role, or `null` when the
//!   credential is the whole of it;
//! - `x-clax-stream`: whether it answers an event stream;
//! - on each error answer, `x-clax-codes`: the `error.code`s it carries.

pub use schemars;
use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The contract's version (semver). Its major is [`MAJOR`]; a change that
/// breaks a client of the last release raises it.
pub const VERSION: &str = "1.0.0";
/// [`VERSION`]'s major, as discovery serves it (`contract_version`).
pub const MAJOR: &str = "1";

/// A schema of a request's or an answer's JSON, from the type that reads or
/// writes it ([`schema`]).
pub type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

/// [`SchemaFn`] of `T`.
pub fn schema<T: JsonSchema>(g: &mut SchemaGenerator) -> Schema {
    g.subschema_for::<T>()
}

/// An operation's HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    /// As OpenAPI and axum's routing name it: `get`, `post`, ….
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "get",
            Method::Post => "post",
            Method::Put => "put",
            Method::Patch => "patch",
            Method::Delete => "delete",
        }
    }
}

/// A kind of credential an operation accepts (spec §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// None: anyone may call it.
    Anonymous,
    /// A browser session: the owner's or a viewer's cookie, from the
    /// instance's own origin.
    Session,
    /// The daemon's bearer token (the owner by hand, or an owner agent's).
    Token,
    /// An agent's signed grant (`clax-ed25519 v2`).
    Grant,
    /// A queue's service grant: `Authorization: Clax-Queue` and its
    /// signature, from a deployer's backend (§5.3).
    Service,
    /// A reporter token: `Authorization: Clax-Reporter`, its key's
    /// signature and its origin, from a deployer's page (§5.3).
    Reporter,
}

impl Channel {
    /// The served name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Anonymous => "anonymous",
            Channel::Session => "session",
            Channel::Token => "token",
            Channel::Grant => "grant",
            Channel::Service => "service",
            Channel::Reporter => "reporter",
        }
    }
}

/// One part of a `multipart/form-data` body.
#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub name: &'static str,
    pub required: bool,
    /// The part's media type: `text/plain`, `application/json`,
    /// `image/png`, `application/jsonl`.
    pub media_type: &'static str,
    /// For an `application/json` part, the schema of its JSON.
    pub json: Option<SchemaFn>,
    pub description: &'static str,
}

/// A request's or an answer's body.
#[derive(Clone, Copy, Debug)]
pub enum Body {
    /// None (`204` for an answer).
    Empty,
    /// `application/json` of the schema.
    Json(SchemaFn),
    /// `multipart/form-data` of these parts.
    Multipart(&'static [Part]),
    /// `image/png` bytes.
    Png,
}

/// What a credential must hold for an operation (`x-clax-rights`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rights {
    /// The credential is the whole of it (a reporter token is its queue's,
    /// its reporter's and its origin's).
    Credential,
    /// A `clax_core::rights` action or an agent gate, by name.
    Action(&'static str),
    /// A role on the path's queue: `admin` or `triage`.
    QueueRole(&'static str),
}

/// A public operation, declared beside its handler.
#[derive(Clone, Copy, Debug)]
pub struct Op {
    /// The `operationId`: unique in the contract, and the name the SDKs
    /// call it by.
    pub id: &'static str,
    /// The handler's function name, which the route table names (checked
    /// by the routes' tests).
    pub handler: &'static str,
    pub method: Method,
    /// axum's path, its parameters `{name}`.
    pub path: &'static str,
    pub summary: &'static str,
    /// The query, as the struct that reads it.
    pub query: Option<SchemaFn>,
    pub request: Body,
    /// The answer's status on success.
    pub status: u16,
    pub response: Body,
    /// The error answers it gives, `(status, code)`, besides those its
    /// channel's credential check gives ([`credential_errors`]).
    pub errors: &'static [(u16, &'static str)],
    pub retry: bool,
    pub channel: &'static [Channel],
    pub rights: Rights,
    pub stream: bool,
}

/// The errors each credential kind's check answers before the handler runs.
#[must_use]
pub fn credential_errors(c: Channel) -> &'static [(u16, &'static str)] {
    match c {
        Channel::Anonymous => &[],
        Channel::Session | Channel::Token | Channel::Grant => &[(401, "unauthorized")],
        // clax_http::reporter and grant_auth.
        Channel::Service => &[
            (401, "invalid_token"),
            (401, "invalid_signature"),
            (401, "stale_request"),
            (401, "replayed_request"),
            (403, "forbidden_origin"),
            (413, "body_too_large"),
            (429, "limit_reached"),
        ],
        Channel::Reporter => &[
            (401, "invalid_token"),
            (401, "token_expired"),
            (401, "invalid_signature"),
            (401, "stale_request"),
            (401, "replayed_request"),
            (403, "wrong_origin"),
            (413, "body_too_large"),
            (429, "limit_reached"),
        ],
    }
}

/// Every error answer's body (`clax_http::error::ApiError`).
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

/// An error answer's `error`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ApiErrorDetail {
    /// What went wrong, for code to branch on: one of the operation's
    /// `x-clax-codes` for its status.
    pub code: String,
    /// What went wrong, for a person.
    pub message: String,
}

fn settings() -> SchemaSettings {
    SchemaSettings::draft2020_12().with(|s| {
        s.definitions_path = "/components/schemas".into();
        s.meta_schema = None;
    })
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        _ => "Error",
    }
}

fn path_params(path: &str) -> Vec<Value> {
    path.split('/')
        .filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(|name| {
            json!({"name": name, "in": "path", "required": true, "schema": {"type": "string"}})
        })
        .collect()
}

/// The query struct's properties as query parameters.
fn query_params(q: SchemaFn) -> Vec<Value> {
    // Inline: a query is its parameters, not a component.
    let mut g = settings()
        .for_deserialize()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let mut s = q(&mut g).to_value();
    optional_is_not_null(&mut s);
    let required: Vec<&str> = s["required"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    if let Some(props) = s["properties"].as_object() {
        for (name, schema) in props {
            let mut schema = schema.clone();
            let description = schema.as_object_mut().and_then(|o| o.remove("description"));
            let mut p = json!({"name": name, "in": "query", "required": required.contains(&name.as_str()), "schema": schema});
            if let Some(d) = description {
                p["description"] = d;
            }
            out.push(p);
        }
    }
    out
}

fn body_content(b: Body, g: &mut SchemaGenerator) -> Option<Value> {
    match b {
        Body::Empty => None,
        Body::Json(f) => Some(json!({"application/json": {"schema": f(g).to_value()}})),
        Body::Png => Some(
            json!({"image/png": {"schema": {"type": "string", "contentMediaType": "image/png"}}}),
        ),
        Body::Multipart(parts) => {
            let mut props = Map::new();
            let mut required = Vec::new();
            let mut encoding = Map::new();
            for p in parts {
                let mut s = json!({"type": "string", "description": p.description});
                if p.media_type != "text/plain" {
                    s["contentMediaType"] = json!(p.media_type);
                }
                if let Some(f) = p.json {
                    s["contentSchema"] = f(g).to_value();
                }
                props.insert(p.name.to_string(), s);
                encoding.insert(p.name.to_string(), json!({"contentType": p.media_type}));
                if p.required {
                    required.push(p.name);
                }
            }
            Some(json!({"multipart/form-data": {
                "schema": {"type": "object", "properties": props, "required": required, "additionalProperties": false},
                "encoding": encoding,
            }}))
        }
    }
}

fn operation(op: &Op, out: &mut SchemaGenerator, input: &mut SchemaGenerator) -> Value {
    let mut params = path_params(op.path);
    if let Some(q) = op.query {
        params.extend(query_params(q));
    }
    let mut responses = Map::new();
    let mut ok = json!({"description": status_text(op.status)});
    if let Some(c) = body_content(op.response, out) {
        ok["content"] = c;
    }
    responses.insert(op.status.to_string(), ok);
    let mut errors: BTreeMap<u16, Vec<&str>> = BTreeMap::new();
    for c in op.channel {
        for (st, code) in credential_errors(*c) {
            errors.entry(*st).or_default().push(code);
        }
    }
    for (st, code) in op.errors {
        errors.entry(*st).or_default().push(code);
    }
    let err_schema = schema::<ApiErrorBody>(out).to_value();
    for (st, mut codes) in errors {
        codes.sort_unstable();
        codes.dedup();
        responses.insert(
            st.to_string(),
            json!({
                "description": status_text(st),
                "x-clax-codes": codes,
                "content": {"application/json": {"schema": err_schema.clone()}},
            }),
        );
    }
    let mut o = json!({
        "operationId": op.id,
        "summary": op.summary,
        "responses": responses,
        "x-clax-retry": op.retry,
        "x-clax-channel": op.channel.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
        "x-clax-rights": match op.rights {
            Rights::Credential => Value::Null,
            Rights::Action(a) => json!({"action": a}),
            Rights::QueueRole(r) => json!({"queue_role": r}),
        },
        "x-clax-stream": op.stream,
    });
    if !params.is_empty() {
        o["parameters"] = Value::Array(params);
    }
    if let Some(c) = body_content(op.request, input) {
        o["requestBody"] = json!({"required": true, "content": c});
    }
    o
}

/// The OpenAPI 3.1 document of `ops`: answers are described as the
/// handlers serialize them, requests as they deserialize them.
///
/// # Panics
/// When two ops share an `operationId` or a method and path.
#[must_use]
pub fn openapi(ops: &[&Op]) -> Value {
    let mut out = settings().for_serialize().into_generator();
    let mut input = settings().for_deserialize().into_generator();
    let mut paths: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    let mut ids = std::collections::BTreeSet::new();
    for op in ops {
        assert!(ids.insert(op.id), "operationId {} is declared twice", op.id);
        let item = paths.entry(op.path).or_default();
        let prev = item.insert(
            op.method.as_str().to_string(),
            operation(op, &mut out, &mut input),
        );
        assert!(
            prev.is_none(),
            "{} {} is declared twice",
            op.method.as_str(),
            op.path
        );
    }
    // A type read differently from how it is written (a field with a
    // default is optional in a request, always present in an answer) is
    // two schemas: the request's is `<Name>Input`.
    let mut schemas = out.take_definitions(true);
    let mut inputs = input.take_definitions(true);
    schemas.values_mut().for_each(optional_is_not_null);
    inputs.values_mut().for_each(optional_is_not_null);
    // So is one whose request schema refers to such a type.
    let mut renamed: Vec<String> = Vec::new();
    loop {
        let more: Vec<String> = inputs
            .iter()
            .filter(|(name, s)| {
                !renamed.contains(name)
                    && schemas.get(*name).is_some_and(|o| {
                        o != *s
                            || renamed
                                .iter()
                                .any(|r| refers_to(s, &format!("#/components/schemas/{r}")))
                    })
            })
            .map(|(name, _)| name.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        renamed.extend(more);
    }
    let rename = |v: &mut Value| {
        for name in &renamed {
            rewrite_ref(
                v,
                &format!("#/components/schemas/{name}"),
                &format!("#/components/schemas/{name}Input"),
            );
        }
    };
    for (name, mut s) in std::mem::take(&mut inputs) {
        rename(&mut s);
        if renamed.contains(&name) {
            schemas.insert(format!("{name}Input"), s);
        } else {
            schemas.entry(name).or_insert(s);
        }
    }
    for item in paths.values_mut() {
        for o in item.values_mut() {
            if let Some(b) = o.get_mut("requestBody") {
                rename(b);
            }
            if let Some(p) = o.get_mut("parameters") {
                rename(p);
            }
        }
    }
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Clax",
            "version": VERSION,
            "description": "The public API of a Clax instance (spec 2026-10-27-clax-sdk-design §4.1), generated from the routes by clax_core::contract. Do not edit: regenerate with `just contract`.",
        },
        "paths": paths,
        "components": {"schemas": schemas},
    })
}

/// An object's optional property (an `Option` serde skips when `None`) is
/// absent rather than `null`: in an answer it is never `null`, and a request
/// leaves it out (Clax reads `null` there as absent too). A required one
/// keeps its `null` (an `Option` always sent).
fn optional_is_not_null(v: &mut Value) {
    let Value::Object(m) = v else { return };
    let required: Vec<String> = m
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if let Some(Value::Object(props)) = m.get_mut("properties") {
        for (name, p) in props.iter_mut() {
            let Value::Object(pm) = p else { continue };
            // `#[serde(default)]` on an `Option`: says no more than optional.
            if pm.get("default") == Some(&Value::Null) {
                pm.remove("default");
            }
            if required.contains(name) {
                continue;
            }
            if let Some(Value::Array(types)) = pm.get("type")
                && types.len() == 2
                && types.contains(&json!("null"))
            {
                let t = types.iter().find(|t| *t != "null").cloned().unwrap();
                pm.insert("type".into(), t);
            }
            if let Some(Value::Array(any)) = pm.get("anyOf")
                && any.len() == 2
                && any.contains(&json!({"type": "null"}))
            {
                let s = any
                    .iter()
                    .find(|s| **s != json!({"type": "null"}))
                    .cloned()
                    .unwrap();
                pm.remove("anyOf");
                if let Value::Object(sm) = s {
                    for (k, x) in sm {
                        pm.entry(k).or_insert(x);
                    }
                }
            }
        }
    }
    for x in m.values_mut() {
        optional_is_not_null(x);
    }
}

/// Whether `v` holds a `$ref` to `target`.
fn refers_to(v: &Value, target: &str) -> bool {
    match v {
        Value::Object(m) => {
            m.get("$ref").and_then(Value::as_str) == Some(target)
                || m.values().any(|x| refers_to(x, target))
        }
        Value::Array(a) => a.iter().any(|x| refers_to(x, target)),
        _ => false,
    }
}

/// Replaces each `$ref` `from` in `v` with `to`.
fn rewrite_ref(v: &mut Value, from: &str, to: &str) {
    match v {
        Value::Object(m) => {
            if m.get("$ref").and_then(Value::as_str) == Some(from) {
                m.insert("$ref".into(), json!(to));
            }
            m.values_mut().for_each(|x| rewrite_ref(x, from, to));
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rewrite_ref(x, from, to)),
        _ => {}
    }
}

/// [`openapi`] as the committed file has it: pretty JSON and a newline.
#[must_use]
pub fn render(ops: &[&Op]) -> String {
    let mut s = serde_json::to_string_pretty(&openapi(ops)).expect("a JSON value serializes");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_major_is_the_versions() {
        assert_eq!(VERSION.split('.').next(), Some(MAJOR));
        assert_eq!(VERSION.split('.').count(), 3);
    }

    #[derive(JsonSchema, serde::Deserialize)]
    #[allow(dead_code)]
    struct Q {
        /// The page.
        cursor: Option<String>,
    }

    #[derive(JsonSchema, serde::Serialize)]
    struct Answer {
        /// Absent when none.
        #[serde(skip_serializing_if = "Option::is_none")]
        maybe: Option<String>,
        /// Always sent, `null` when none.
        nullable: Option<String>,
    }

    const OP: Op = Op {
        id: "test_get",
        handler: "get",
        method: Method::Get,
        path: "/api/things/{id}",
        summary: "A thing.",
        query: Some(schema::<Q>),
        request: Body::Empty,
        status: 200,
        response: Body::Json(schema::<Answer>),
        errors: &[(404, "not_found")],
        retry: true,
        channel: &[Channel::Reporter],
        rights: Rights::Credential,
        stream: false,
    };

    #[test]
    fn an_op_becomes_an_operation() {
        let doc = openapi(&[&OP]);
        let o = &doc["paths"]["/api/things/{id}"]["get"];
        assert_eq!(o["operationId"], "test_get");
        assert_eq!(o["x-clax-retry"], true);
        assert_eq!(o["x-clax-channel"], json!(["reporter"]));
        assert_eq!(o["x-clax-rights"], Value::Null);
        assert_eq!(o["parameters"][0]["name"], "id");
        assert_eq!(o["parameters"][1]["name"], "cursor");
        assert_eq!(o["parameters"][1]["required"], false);
        assert_eq!(o["responses"]["404"]["x-clax-codes"], json!(["not_found"]));
        assert!(
            o["responses"]["401"]["x-clax-codes"]
                .as_array()
                .unwrap()
                .contains(&json!("invalid_token"))
        );
        assert_eq!(
            o["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Answer"
        );
        let a = &doc["components"]["schemas"]["Answer"];
        // A field skipped when none is optional; one always sent is
        // required, and may be null.
        assert_eq!(a["required"], json!(["nullable"]));
        assert_eq!(
            a["properties"]["nullable"]["type"],
            json!(["string", "null"])
        );
    }
}

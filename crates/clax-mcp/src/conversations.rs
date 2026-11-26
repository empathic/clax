//! The tools of Antigravity's shim. An `agy` process starts one MCP server
//! and shares it between its conversations and their subagents, naming the
//! caller in each `tools/call`'s `_meta`; each root conversation gets a Clax
//! session of its own, and a subagent acts in its root's.

use crate::client::DaemonClient;
use crate::tools::ClaxTools;
use clax_core::RegisterSession;
use clax_core::antigravity::{META_CONVERSATION, META_ROOT};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, RequestMetaObject, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Who made a tool call, from its `_meta`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub root: String,
    pub subagent: bool,
}

/// `None` when neither key holds a non-empty string. `root` is META_ROOT,
/// else META_CONVERSATION; `subagent` when both are present and differ.
pub fn caller(meta: &RequestMetaObject) -> Option<Caller> {
    let key = |k: &str| {
        meta.get(k)
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
    };
    let (root, conversation) = (key(META_ROOT), key(META_CONVERSATION));
    Some(Caller {
        root: root.or(conversation)?.to_string(),
        subagent: root.zip(conversation).is_some_and(|(r, c)| r != c),
    })
}

/// The tools of one `agy` process: a Clax session per root conversation.
pub struct Conversations {
    template: ClaxTools,
    base: RegisterSession,
    connect: Arc<dyn Fn(RegisterSession) -> DaemonClient + Send + Sync>,
    heartbeat: Duration,
    /// By root conversation ID; `None` holds the calls that name none.
    held: Mutex<BTreeMap<Option<String>, Conversation>>,
}

struct Conversation {
    tools: ClaxTools,
    beat: tokio::task::JoinHandle<()>,
}

impl Conversations {
    /// The tools of each conversation are `template`'s, on a client
    /// `connect` makes for `base` with the conversation's root ID as its
    /// harness session ID, heartbeated every `heartbeat`.
    pub fn new(
        template: ClaxTools,
        base: RegisterSession,
        connect: Arc<dyn Fn(RegisterSession) -> DaemonClient + Send + Sync>,
        heartbeat: Duration,
    ) -> Conversations {
        Conversations {
            template,
            base,
            connect,
            heartbeat,
            held: Mutex::default(),
        }
    }

    /// The tools of `root`'s session (of the session with no harness
    /// session ID for `None`), made and heartbeated on first use.
    pub fn tools_for(&self, root: Option<&str>) -> ClaxTools {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        held.entry(root.map(str::to_string))
            .or_insert_with(|| {
                let client = (self.connect)(RegisterSession {
                    harness_session_id: root.map(str::to_string),
                    ..self.base.clone()
                });
                let beat = tokio::spawn(crate::shim::beat(client.clone(), self.heartbeat));
                Conversation {
                    tools: self.template.for_client(client),
                    beat,
                }
            })
            .tools
            .clone()
    }

    /// Settles reports and ends every session, each within `within`;
    /// returns the root conversation IDs it held.
    pub async fn end_all(&self, within: Duration) -> Vec<String> {
        let held = std::mem::take(&mut *self.held.lock().unwrap_or_else(|e| e.into_inner()));
        let mut ending = tokio::task::JoinSet::new();
        let mut roots = Vec::new();
        for (root, c) in held {
            c.beat.abort();
            roots.extend(root);
            ending.spawn(async move { crate::shim::end(&c.tools, within).await });
        }
        ending.join_all().await;
        roots
    }
}

impl ServerHandler for Conversations {
    /// Runs the call with its caller's tools: its root conversation's,
    /// without feedback for a subagent.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let who = caller(&context.meta).or_else(|| request.meta.as_ref().and_then(caller));
        let tools = self.tools_for(who.as_ref().map(|c| c.root.as_str()));
        let tools = match who {
            Some(c) if c.subagent => tools.without_feedback(),
            _ => tools,
        };
        tools.call_tool(request, context).await
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        self.template.list_tools(request, context).await
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.template.get_tool(name)
    }

    fn get_info(&self) -> ServerConfig {
        self.template.get_info()
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        self.template.supported_protocol_versions()
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use clax_core::antigravity::{META_CONVERSATION, META_ROOT};
    use serde_json::{Value, json};

    fn meta(v: Value) -> RequestMetaObject {
        let Value::Object(m) = v else {
            panic!("meta is an object")
        };
        m.into()
    }

    fn of(v: Value) -> Option<(String, bool)> {
        caller(&meta(v)).map(|c| (c.root, c.subagent))
    }

    #[test]
    fn a_root_conversation_names_itself_twice() {
        assert_eq!(
            of(json!({META_ROOT: "r1", META_CONVERSATION: "r1", "progressToken": "t"})),
            Some(("r1".into(), false))
        );
    }

    #[test]
    fn a_subagent_names_its_root() {
        assert_eq!(
            of(json!({META_ROOT: "r1", META_CONVERSATION: "s1"})),
            Some(("r1".into(), true))
        );
    }

    #[test]
    fn either_key_alone_is_the_root() {
        assert_eq!(of(json!({META_ROOT: "r1"})), Some(("r1".into(), false)));
        assert_eq!(
            of(json!({META_CONVERSATION: "c1"})),
            Some(("c1".into(), false))
        );
    }

    #[test]
    fn no_usable_key_is_no_caller() {
        assert_eq!(of(json!({"progressToken": "t"})), None);
        assert_eq!(of(json!({META_ROOT: "", META_CONVERSATION: ""})), None);
        assert_eq!(of(json!({META_ROOT: 1, META_CONVERSATION: null})), None);
        assert_eq!(
            of(json!({META_ROOT: "", META_CONVERSATION: "c1"})),
            Some(("c1".into(), false))
        );
    }
}

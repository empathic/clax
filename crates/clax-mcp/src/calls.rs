//! Each MCP tool call's identity (spec 2026-10-06-toolpath-audit-design
//! §6.7): a call ID minted when the call arrives, the argument hash
//! (§12.2) of the arguments as received, and, for a tool that changes
//! history, the agent's git state (§9.3). Every request made for the call
//! carries them in `x-clax-call` and `x-clax-git`; once the result has gone
//! back, the call is reported in the background.

use clax_core::audit::{CallHeader, ToolCallReport, ToolOutcome, encode_call_header};
use clax_core::gitctx::{self, GitField};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// The tools whose calls capture git state (spec §9.3): every tool that
/// changes history. The read-only tools, and `open`, which only shows a
/// page, capture none.
pub const GIT_TOOLS: [&str; 15] = [
    "publish",
    "comments_reply",
    "comments_resolve",
    "watch",
    "asset_upload",
    "db_set",
    "db_update",
    "db_delete",
    "db_str_replace",
    "db_batch",
    "delete",
    "pin",
    "unpin",
    "working",
    "ask",
];

/// Whether a call of `tool` captures git state.
pub fn captures_git(tool: &str) -> bool {
    GIT_TOOLS.contains(&tool)
}

/// The git state of one call: captured once, in the background, from when
/// the call arrives, so the capture overlaps the tool's own work. The
/// capture runs in a task of its own that publishes its result, so a
/// request that stops waiting never loses it for the call's later ones.
struct GitSlot(watch::Receiver<Option<GitField>>);

impl GitSlot {
    fn ready(field: GitField) -> GitSlot {
        GitSlot(watch::channel(Some(field)).1)
    }

    fn pending(task: JoinHandle<GitField>) -> GitSlot {
        let (tx, rx) = watch::channel(None);
        tokio::spawn(async move {
            let field = task.await.unwrap_or(GitField::Capture("unavailable"));
            let _ = tx.send(Some(field));
        });
        GitSlot(rx)
    }

    /// The captured state, waiting for the capture (bounded by its own
    /// deadline). A capture that failed to finish is `unavailable`.
    async fn get(&self) -> GitField {
        let mut rx = self.0.clone();
        match rx.wait_for(Option::is_some).await {
            Ok(v) => v.clone().unwrap_or(GitField::Capture("unavailable")),
            Err(_) => GitField::Capture("unavailable"),
        }
    }

    /// The captured state, if the capture has already finished.
    fn now(&self) -> Option<GitField> {
        self.0.borrow().clone()
    }
}

/// One tool call in progress.
pub struct CallScope {
    header: CallHeader,
    encoded: Option<String>,
    git: Option<GitSlot>,
    artifact: Mutex<Option<String>>,
}

impl std::fmt::Debug for CallScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallScope")
            .field("call_id", &self.header.call_id)
            .field("tool", &self.header.tool)
            .finish()
    }
}

impl CallScope {
    /// A call of `tool` with `arguments` (absent arguments are `{}`),
    /// starting now, with a new call ID. `git` is the call's capture, for a
    /// tool that captures one: a task still running, or a state already
    /// known.
    pub fn begin(
        tool: &str,
        arguments: Option<&serde_json::Map<String, serde_json::Value>>,
        git: Option<GitStart>,
    ) -> CallScope {
        let args_sha256 = match arguments {
            Some(a) => clax_core::toolpath::args::object_sha256(a),
            None => clax_core::toolpath::args::object_sha256(&serde_json::Map::new()),
        };
        let header = CallHeader {
            call_id: clax_core::new_ulid(),
            tool: tool.to_string(),
            harness_tool: None,
            args_sha256,
            started_at: clax_core::Store::now(),
            harness_call_id: None,
        };
        let encoded = encode_call_header(&header).ok();
        CallScope {
            header,
            encoded,
            git: git.map(|g| match g {
                GitStart::Ready(f) => GitSlot::ready(f),
                GitStart::Running(t) => GitSlot::pending(t),
            }),
            artifact: Mutex::new(None),
        }
    }

    /// The call's identity.
    pub fn header(&self) -> &CallHeader {
        &self.header
    }

    /// The `x-clax-call` value; `None` when the call's identity does not
    /// fit a header (a tool name that is not a bare Clax name).
    pub fn call_header(&self) -> Option<&str> {
        self.encoded.as_deref()
    }

    /// Whether the call carries git state.
    pub fn has_git(&self) -> bool {
        self.git.is_some()
    }

    /// The `x-clax-git` value, waiting for the capture.
    pub async fn git_header(&self) -> Option<String> {
        gitctx::encode_header(&self.git.as_ref()?.get().await)
    }

    /// The `x-clax-git` value if the capture has finished.
    pub fn git_header_now(&self) -> Option<String> {
        gitctx::encode_header(&self.git.as_ref()?.now()?)
    }

    /// Notes the artifact the call's arguments named; the first one noted
    /// is kept.
    pub fn note_artifact(&self, id: &str) {
        let mut a = self.artifact.lock().unwrap_or_else(|e| e.into_inner());
        if a.is_none() {
            *a = Some(id.to_string());
        }
    }

    /// The report of this call, ending now with `outcome`.
    pub fn report(&self, outcome: ToolOutcome) -> ToolCallReport {
        let mut r = ToolCallReport::new(self.header.clone(), clax_core::Store::now(), outcome);
        r.artifact_id = self
            .artifact
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        r
    }
}

/// How a call's git capture starts.
pub enum GitStart {
    /// Already known (no working directory, say).
    Ready(GitField),
    /// Running on a blocking thread.
    Running(JoinHandle<GitField>),
}

/// The tool calls still running and the reports still being sent, so a
/// shim that is closing can let them finish.
#[derive(Clone)]
pub struct Reports {
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    running: Arc<watch::Sender<usize>>,
}

impl Default for Reports {
    fn default() -> Reports {
        Reports {
            tasks: Arc::default(),
            running: Arc::new(watch::channel(0).0),
        }
    }
}

/// A tool call counted as running until dropped.
pub struct Running(Arc<watch::Sender<usize>>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n = n.saturating_sub(1));
    }
}

impl Reports {
    /// Counts a call as running until the returned guard drops.
    pub fn running(&self) -> Running {
        self.running.send_modify(|n| *n += 1);
        Running(self.running.clone())
    }

    /// Keeps `task`, forgetting the ones already finished.
    pub fn push(&self, task: JoinHandle<()>) {
        let mut v = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        v.retain(|t| !t.is_finished());
        v.push(task);
    }

    /// Waits for every running call to end and every report to be sent.
    pub async fn settle(&self) {
        let _ = self.running.subscribe().wait_for(|n| *n == 0).await;
        let tasks: Vec<_> =
            std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|e| e.into_inner()));
        for t in tasks {
            let _ = t.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_that_changes_history_captures_git() {
        let tools = crate::ClaxTools::tools();
        assert_eq!(tools.len(), 24);
        for t in &tools {
            let name = t.name.as_ref();
            let read_only = t
                .annotations
                .as_ref()
                .and_then(|a| a.read_only_hint)
                .unwrap_or(false);
            let header = CallScope::begin(name, None, None);
            assert!(
                header.call_header().is_some(),
                "{name} is not a bare tool name"
            );
            if read_only || name == "open" {
                assert!(!captures_git(name), "{name} is read-only");
            } else {
                assert!(captures_git(name), "{name} changes history");
            }
        }
        for name in GIT_TOOLS {
            assert!(tools.iter().any(|t| t.name == name), "{name} is no tool");
        }
    }

    #[test]
    fn a_call_hashes_its_arguments_as_received() {
        let args: serde_json::Value =
            serde_json::from_str(r#"{"n":1.0,"m":-0.5,"big":100000000000000000000}"#).unwrap();
        let c = CallScope::begin("publish", args.as_object(), None);
        assert_eq!(
            c.header().args_sha256,
            "sha256:ed4f6fe44f96cbbe62384feebf79cb9dbc03ff889ae627811e31bd2ea5b2b557"
        );
        let none = CallScope::begin("list", None, None);
        assert_eq!(
            none.header().args_sha256,
            "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        assert!(clax_core::ids::is_ulid(&none.header().call_id));
        assert_ne!(c.header().call_id, none.header().call_id);
        none.note_artifact("a1");
        none.note_artifact("a2");
        let r = none.report(ToolOutcome::Ok);
        assert_eq!(r.artifact_id.as_deref(), Some("a1"));
        assert_eq!(r.validate(), Ok(()));
    }
}

#[cfg(test)]
mod measure {
    use super::*;

    /// The shim's own work per call, before the tool runs: minting the
    /// call ID, hashing the arguments and encoding the header. Run by hand:
    /// `cargo test -p clax-mcp --release --lib -- --ignored
    /// call_scope_cost --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn call_scope_cost() {
        let small = serde_json::json!({"url_or_id": "k3m9q2w8x1ab", "limit": 5});
        let page = format!(
            "<title>Big</title>{}",
            "<p>Lorem ipsum \"dolor\"\n".repeat(44_000)
        );
        let big = serde_json::json!({"html": page, "note": "big"});
        let raw = page.len();
        let t = std::time::Instant::now();
        for _ in 0..10 {
            std::hint::black_box(clax_core::audit::sha256_hex(page.as_bytes()));
        }
        println!(
            "plain SHA-256 of {raw} bytes: {:.0} us",
            t.elapsed().as_secs_f64() * 1e5
        );
        for (name, args) in [("small", &small), ("1 MiB publish", &big)] {
            let mut us: Vec<f64> = (0..50)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let c = CallScope::begin("publish", args.as_object(), None);
                    std::hint::black_box(c.call_header());
                    t.elapsed().as_secs_f64() * 1e6
                })
                .collect();
            us.sort_by(f64::total_cmp);
            println!("{name}: p50 {:.0} us, p95 {:.0} us", us[24], us[47]);
        }
    }
}

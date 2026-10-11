//! Every agent channel, driven through its real client against the test
//! daemon (spec §8.5, §8.6): the built `clax` binary as the MCP shim (stdio),
//! as `clax hook --agent claude stop|prompt|session-start` with harness
//! JSON on stdin, as `clax feedback follow --once`, and as the plain CLI
//! with each harness's shell variables; `/mcp` over HTTP; the Pi extension
//! under Node; and the context of every plugin slash command, run through
//! the real `ensure-clax.sh`. Each returns everything it printed.
//!
//! The clients find the daemon through the `daemon.json` this writes into
//! its home, naming this process and the daemon's version, so none of them
//! starts or replaces a daemon. The owner's agent is the session the world
//! registered for it (harness session ID `owner-h1`), which watches Mia's
//! artifact.

use crate::common::people::World;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// Longest a client may run.
const LIMIT: Duration = Duration::from_secs(30);

/// What one channel printed.
#[derive(Debug)]
pub struct Out {
    pub channel: String,
    pub text: String,
}

/// The `clax` binary: `CLAX_TEST_BIN`, else a copy of this build's kept by
/// content (`scripts/stable-bin.sh`), never `target/debug/clax` run in place
/// (macOS assesses each new executable, and a build would replace it under
/// a run).
pub fn clax_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(bin) = std::env::var_os("CLAX_TEST_BIN") {
            let bin = PathBuf::from(bin);
            assert!(bin.exists(), "CLAX_TEST_BIN {} missing", bin.display());
            return bin;
        }
        let workspace = repo();
        let status = std::process::Command::new(env!("CARGO"))
            .current_dir(&workspace)
            .args(["build", "--quiet", "-p", "clax-cli", "--bin", "clax"])
            .status()
            .expect("run cargo build");
        assert!(status.success(), "cargo build -p clax-cli failed");
        let exe = std::env::current_exe().unwrap();
        let built = exe.parent().unwrap().parent().unwrap().join("clax");
        let store = built
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("clax-bin/debug");
        let out = std::process::Command::new(workspace.join("scripts/stable-bin.sh"))
            .arg(&built)
            .arg(&store)
            .output()
            .expect("run scripts/stable-bin.sh");
        assert!(
            out.status.success(),
            "stable-bin.sh: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let kept = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
        if kept.is_absolute() {
            kept
        } else {
            workspace.join(kept)
        }
    })
    .clone()
}

/// The repository's root.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The variables a harness sets in its shell (spec §16, as the probe saw
/// them), and the Pi extension's and the wrapper's marker.
pub const HARNESS_ENVS: &[(&str, &str)] = &[
    ("CLAUDECODE", "1"),
    ("CODEX_THREAD_ID", "019a0000-0000-7000-8000-000000000000"),
    ("CODEX_CI", "1"),
    ("CODEX_SANDBOX", "seatbelt"),
    ("PI_CODING_AGENT", "true"),
    ("CLAX_CHANNEL", "agent"),
];

/// The real clients of a [`World`].
pub struct RealClients<'a> {
    w: &'a World,
    scratch: tempfile::TempDir,
}

impl<'a> RealClients<'a> {
    pub fn new(w: &'a World) -> RealClients<'a> {
        let info = clax_server::daemon::DaemonInfo {
            port: w.ts.addr.port(),
            pid: std::process::id(),
            token: w.ts.token.clone(),
            started_at: w.ts.store.now(),
            bind: "127.0.0.1".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            exe: None,
            commit: None,
        };
        clax_server::daemon::write_daemon_info(&w.ts.home, &info).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(scratch.path().join("work")).unwrap();
        RealClients { w, scratch }
    }

    /// `program args` with a cleared environment pointing at the daemon's
    /// home, plus `env`.
    fn command(
        &self,
        program: &Path,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(program);
        c.args(args)
            .env_clear()
            .env(
                "PATH",
                "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin",
            )
            .env("HOME", self.scratch.path())
            .env("TMPDIR", self.scratch.path())
            .env("CLAX_HOME", self.w.ts.home.root())
            .env("CLAX_NO_OPEN", "1")
            .env("CLAX_CODEX_BIN", "")
            .env("RUST_LOG", "error")
            .current_dir(self.scratch.path().join("work"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in env {
            c.env(k, v);
        }
        c
    }

    /// Runs `cmd` with `stdin`: its stdout, stderr and exit status.
    async fn run(&self, mut cmd: tokio::process::Command, stdin: &str) -> (String, i32) {
        use tokio::io::AsyncWriteExt;
        let what = format!("{:?}", cmd.as_std());
        let mut child = cmd.spawn().expect("spawn a client");
        let mut input = child.stdin.take().unwrap();
        input.write_all(stdin.as_bytes()).await.unwrap();
        drop(input);
        let out = tokio::time::timeout(LIMIT, child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("{what} did not finish within {LIMIT:?}"))
            .unwrap();
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (text, out.status.code().unwrap_or(-1))
    }

    /// `clax <args>` with `env`: what it printed, and its exit status.
    pub async fn clax(&self, args: &[&str], env: &[(&str, &str)]) -> (String, i32) {
        self.run(self.command(&clax_bin(), args, env), "").await
    }

    /// Every channel but Pi's ([`RealClients::pi`]), each driven through
    /// its real client. The owner's comment on Mia's page is sent first, so
    /// the notices and the feedback tiers have something to hand over.
    pub async fn run_every_channel(&self) -> Vec<Out> {
        self.w.owner_thread_on_mias_page().await;
        // The notices first: the hooks hand the feedback over after them.
        let mut out = vec![self.follow().await];
        out.extend(self.hooks().await);
        out.push(self.shim().await);
        out.push(self.mcp_http().await);
        out.extend(self.cli_under_harnesses().await);
        out.extend(self.slash_command_context().await);
        out
    }

    /// The Claude Code hooks of the owner's agent's session.
    pub async fn hooks(&self) -> Vec<Out> {
        let mut out = Vec::new();
        for event in ["session-start", "prompt", "stop"] {
            out.push(self.hook(event).await);
        }
        out
    }

    /// One Claude Code hook (`session-start`, `prompt` or `stop`) of the
    /// owner's agent's session.
    pub async fn hook(&self, event: &str) -> Out {
        let name = match event {
            "session-start" => "SessionStart",
            "prompt" => "UserPromptSubmit",
            _ => "Stop",
        };
        let input = json!({"session_id": "owner-h1", "hook_event_name": name,
            "stop_hook_active": false, "prompt": "Carry on.", "cwd": "/w", "source": "startup"})
        .to_string();
        let cmd = self.command(&clax_bin(), &["hook", "--agent", "claude", event], &[]);
        let (text, _) = self.run(cmd, &input).await;
        Out {
            channel: format!("clax hook --agent claude {event}"),
            text,
        }
    }

    /// `clax feedback follow --once` for the owner's agent's session.
    pub async fn follow(&self) -> Out {
        let (text, _) = self
            .clax(
                &[
                    "feedback",
                    "follow",
                    "--session",
                    &self.w.owner_session,
                    "--once",
                    "--poll-secs",
                    "1",
                ],
                &[],
            )
            .await;
        Out {
            channel: "clax feedback follow --once".into(),
            text,
        }
    }

    /// The MCP shim (`clax mcp --agent claude`, stdio) of the owner's
    /// agent's session: the reading tools on Mia's artifact.
    pub async fn shim(&self) -> Out {
        let aid = &self.w.read.artifact;
        let mut lines = vec![
            json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18",
                "capabilities": {}, "clientInfo": {"name": "canary", "version": "0"}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        ];
        for (i, (name, args)) in tool_calls(aid).into_iter().enumerate() {
            lines.push(
                json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call",
                "params": {"name": name, "arguments": args}}),
            );
        }
        let n = lines.len() - 2;
        let cmd = self.command(
            &clax_bin(),
            &["mcp", "--agent", "claude"],
            &[("CLAUDE_CODE_SESSION_ID", "owner-h1")],
        );
        let text = rpc_over_stdio(cmd, &lines, n).await;
        Out {
            channel: "the MCP shim".into(),
            text,
        }
    }

    /// The daemon's `/mcp` over HTTP: the same tools.
    pub async fn mcp_http(&self) -> Out {
        let ts = &self.w.ts;
        let post = |body: Value, session: Option<&str>| {
            let mut r = ts
                .authed(ts.client.post(format!("{}/mcp", ts.base)))
                .header("accept", "application/json, text/event-stream")
                .json(&body);
            if let Some(s) = session {
                r = r.header("mcp-session-id", s);
            }
            r
        };
        let mut text = String::new();
        let res = post(
            json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18",
                "capabilities": {}, "clientInfo": {"name": "canary", "version": "0"}}}),
            None,
        )
        .send()
        .await
        .unwrap();
        let session = res
            .headers()
            .get("mcp-session-id")
            .map(|v| v.to_str().unwrap().to_string());
        text.push_str(&res.text().await.unwrap());
        post(
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            session.as_deref(),
        )
        .send()
        .await
        .unwrap();
        for (i, (name, args)) in tool_calls(&self.w.read.artifact).into_iter().enumerate() {
            let res = post(
                json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call",
                    "params": {"name": name, "arguments": args}}),
                session.as_deref(),
            )
            .send()
            .await
            .unwrap();
            text.push_str(&res.text().await.unwrap());
            text.push('\n');
        }
        Out {
            channel: "/mcp".into(),
            text,
        }
    }

    /// The plain `clax` binary, no wrapper, under each harness's shell
    /// variables: every command that reads what an agent sees (the list,
    /// comments, `read` of each file kind, versions, the page database,
    /// status), and an asset upload, whose answer an agent reads too.
    pub async fn cli_under_harnesses(&self) -> Vec<Out> {
        let aid = self.w.read.artifact.clone();
        let scratch = self.w.scratch.artifact.clone();
        let upload = self.scratch.path().join("upload.txt");
        std::fs::write(&upload, "an upload").unwrap();
        let upload = upload.to_str().unwrap().to_string();
        let commands: Vec<Vec<String>> = [
            vec!["--json", "list"],
            vec!["comments", "--all"],
            vec!["comments", &aid],
            vec!["--json", "read", &aid],
            vec![
                "--json",
                "read",
                &aid,
                "--version",
                "1",
                "--path",
                "MIACANARYfilename.html",
            ],
            vec![
                "--json",
                "read",
                &aid,
                "--version",
                "2",
                "--path",
                "MIACANARYcarried.css",
            ],
            vec!["--json", "versions", &aid],
            vec!["--json", "db", "list", &aid, "notes"],
            vec!["--json", "db", "get", &aid, "notes", "MIACANARYdocpath"],
            vec!["--json", "db", "query", &aid, "notes"],
            vec!["--json", "status"],
            vec!["--json", "asset", "upload", &scratch, &upload],
        ]
        .into_iter()
        .map(|c| c.into_iter().map(str::to_string).collect())
        .collect();
        let mut out = Vec::new();
        for (k, v) in HARNESS_ENVS {
            for args in &commands {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let (text, _) = self.clax(&args, &[(k, v)]).await;
                out.push(Out {
                    channel: format!("{k}={v} clax {}", args.join(" ")),
                    text,
                });
            }
        }
        out
    }

    /// The context every plugin slash command's `!` lines give the model,
    /// each run through the real wrapper (Claude Code's `/clax:*`).
    pub async fn slash_command_context(&self) -> Vec<Out> {
        let root = repo().join("plugins/claude-code");
        let mut out = Vec::new();
        let mut files: Vec<PathBuf> = std::fs::read_dir(root.join("commands"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            for (i, _) in text.match_indices("!`") {
                let line = &text[i + 2..];
                let cmd = &line[..line.find('`').unwrap()];
                let bash = Path::new("/bin/bash");
                let c = self.command(
                    bash,
                    &["-c", cmd],
                    &[
                        ("CLAUDE_PLUGIN_ROOT", root.to_str().unwrap()),
                        ("CLAX_BIN", clax_bin().to_str().unwrap()),
                        ("CLAUDECODE", "1"),
                    ],
                );
                let (printed, _) = self.run(c, "").await;
                out.push(Out {
                    channel: format!("{}: {cmd}", f.file_name().unwrap().to_string_lossy()),
                    text: printed,
                });
            }
        }
        out
    }

    /// The Pi extension under Node, loaded by Pi's own TypeScript loader
    /// (jiti): its tools, `/clax` command and feedback messages for a Pi
    /// session.
    pub async fn pi(&self) -> Out {
        let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pi-agent.mjs");
        let ext = repo().join("plugins/pi/src/clax.ts");
        let node = which("node").expect("node on PATH");
        let mut c = self.command(
            &node,
            &[
                "--no-deprecation",
                "--import",
                "jiti/register",
                driver.to_str().unwrap(),
                ext.canonicalize().unwrap().to_str().unwrap(),
                self.w.ts.home.root().to_str().unwrap(),
                "pi-canary",
                &self.w.read.artifact,
            ],
            &[("CLAX_BIN", clax_bin().to_str().unwrap())],
        );
        c.current_dir(repo().join("plugins/pi"));
        let (text, _) = self.run(c, "").await;
        Out {
            channel: "the Pi extension".into(),
            text,
        }
    }
}

/// The reading tools an agent calls on artifact `aid`.
fn tool_calls(aid: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("status", json!({})),
        ("list", json!({})),
        ("read", json!({"url_or_id": aid})),
        (
            "read",
            json!({"url_or_id": aid, "version": 1, "path": "MIACANARYfilename.html"}),
        ),
        (
            "read",
            json!({"url_or_id": aid, "version": 2, "path": "MIACANARYcarried.css"}),
        ),
        (
            "comments_read",
            json!({"url_or_id": aid, "include_resolved": true}),
        ),
        ("db_list", json!({"url_or_id": aid, "collection": "notes"})),
        ("wait_for_feedback", json!({"timeout_s": 1})),
    ]
}

/// Writes `lines` to `cmd`'s stdin one by one, each request after the
/// previous answer; everything it printed once `n` answers came (or it
/// stopped).
async fn rpc_over_stdio(mut cmd: tokio::process::Command, lines: &[Value], n: usize) -> String {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut child = cmd.spawn().expect("spawn the shim");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut text = String::new();
    let mut answered = 0;
    let talk = async {
        for l in lines {
            stdin.write_all(format!("{l}\n").as_bytes()).await.unwrap();
            if l.get("id").is_none() {
                continue;
            }
            while let Ok(Some(line)) = stdout.next_line().await {
                text.push_str(&line);
                text.push('\n');
                if serde_json::from_str::<Value>(&line).is_ok_and(|v| v.get("id") == l.get("id")) {
                    if l["method"] == "tools/call" {
                        answered += 1;
                    }
                    break;
                }
            }
        }
    };
    let _ = tokio::time::timeout(LIMIT, talk).await;
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    assert_eq!(answered, n, "the shim answered every call: {text}");
    text
}

/// `name` on this process's `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(name))
            .find(|f| f.is_file())
    })
}

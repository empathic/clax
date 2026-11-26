# Clax for Antigravity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native Clax plugin for Google's Antigravity CLI (`agy`): the Clax tools act in a session per conversation, comments reach a conversation through hooks and wake an idle one through a background `wait_for_feedback`, and `clax init`, `clax doctor` and the docs know the fifth harness.

**Architecture:** One new harness name, `antigravity`, wired through the places Grok is. The shim differs from the other harnesses' in one way: it holds a managed `DaemonClient` and `ClaxTools` per root conversation, chosen from each call's `_meta`, instead of one per process. Hooks reuse the existing events and are translated to Antigravity's input and output shapes at the edges. The wake is today's `wait_for_feedback` with its cap lifted in an interactive run.

**Tech Stack:** Rust (workspace crates `clax-core`, `clax-hooks`, `clax-mcp`, `clax-commands`, `clax-server`, `clax-store`, `clax-http`, `clax-embedded`), rmcp 3.5, POSIX sh (`scripts/tool-hook.sh`, `scripts/ensure-clax.sh`), Bazel through `just`.

**Spec:** `docs/superpowers/specs/2026-11-26-antigravity-plugin-design.md` (read it whole before any task; section 2 is what Antigravity does, Appendix A has the payloads the fixtures come from).

## Global Constraints

- `AGENTS.md` whole. In particular: `qualifier threads <path>` before editing a file; `just test-affected` while working and `just check` once at the end (never `cargo` or `pnpm` directly in a worktree); tests have no sleeps and no wall-clock budgets, and a new `#[cfg(test)]` module carries `#[warn(clippy::disallowed_methods)]`; never run a build against the owner's `~/.clax` or the daemon on 7480; commit with `git -c commit.gpgsign=false commit`, with no AI attribution line.
- Work in a linked worktree on a branch `antigravity-plugin`.
- Exact values, used verbatim everywhere: harness and agent name `antigravity`; plugin `clax-antigravity`; MCP server key `clax`, so the server's run-time name is `clax-antigravity_clax`; permission rule `mcp(clax-antigravity_clax/*)`; minimum `agy` 1.5.18; `_meta` keys `antigravity.google/root_conversation_id` and `antigravity.google/conversation_id`.
- No `SessionStart` hook and no `PreToolUse` hook in the plugin (spec 2.4, 6.3).
- No store migration. No change to another harness's behaviour, except that a Claude Code hook Antigravity runs is logged as `host=antigravity`, not `host=grok`.
- Every hook prints nothing and exits 0 when no `clax` can run or no daemon is running.
- Comments in code are sparse: a line or two only where the code would mislead. Doc comments state the contract.
- Write "ID" in prose.

## Review Focus

1. A `tools/call` whose `_meta` conversation keys are present but empty or not strings: treated as absent, no session keyed by `""`, no panic. Test in Task 5.
2. A conversation ID that is not `[A-Za-z0-9._-]+` reaching a file name (`run/tool-hook/antigravity-<ID>`, `run/wake-nudge/antigravity-<ID>.json`): no file is created or removed outside those directories. Tests in Tasks 3 and 5.
3. `PreInvocation` input with `invocationNum` absent (a zero the encoder left out): treated as the first model call of a turn. Test in Task 3.
4. `~/.gemini/antigravity-cli/settings.json` missing, empty, not JSON, or with `permissions` or `allow` of the wrong type: a yes creates or extends it only when it parses; otherwise the file is untouched and the result says why. Test in Task 9.
5. An interactive run whose initial prompt mentions `-p` (`agy -i "explain the -p flag"`): not taken for a headless run. Test in Task 1.

## Qualifier threads

Read before the task that touches the file; none is closed by this plan.

- `af554733` (`crates/clax-http/src/surface.rs`): how a turn start is inferred. Task 4 adds a harness to that inference.
- `134d9b31` (`crates/clax-hooks/src/events.rs`): which watches count toward the turn-end reminder. Task 3 extends the reminder to Antigravity unchanged.
- `50e69a0f` (`scripts/ensure-clax.sh`): the wrapper's start-up time. Antigravity's connect limit is off (spec waiver on `timeoutSeconds`).
- `bd6918c4` (`crates/clax-commands/src/channel.rs`): Grok's shell variable is unconfirmed. Task 11 adds Antigravity's, which was observed.
- The fifteen threads on the spec are rejected options and accepted risks: leave them open.

---

### Task 1: Names and the headless check

**Files:**
- Create: `crates/clax-core/src/antigravity.rs`
- Modify: `crates/clax-core/src/lib.rs` (declare the module)

**Interfaces:**
- Produces (a command line is a process's argv as `clax_mcp::channel::process_argv` returns it: on macOS `ps` output split on whitespace, so a quoted prompt arrives as several words):

```rust
pub const HARNESS: &str = "antigravity";
pub const PLUGIN: &str = "clax-antigravity";
pub const SERVER: &str = "clax-antigravity_clax";
pub const PERMISSION_RULE: &str = "mcp(clax-antigravity_clax/*)";
pub const MIN_AGY: (u64, u64, u64) = (1, 5, 18);
pub const META_ROOT: &str = "antigravity.google/root_conversation_id";
pub const META_CONVERSATION: &str = "antigravity.google/conversation_id";

/// Whether `argv` runs `agy`: the file name of its first word is `agy`,
/// or of its second (a script named `agy` run by an interpreter).
pub fn is_agy(argv: &[String]) -> bool;
/// Whether an `agy` command line is a headless run. Scans the words after
/// `agy` left to right: `-p`, `--print`, `--prompt` (or `--print=…`,
/// `--prompt=…`) first means headless; `-i` or `--prompt-interactive` first
/// means interactive; neither means interactive.
pub fn headless(argv: &[String]) -> bool;
/// The nearest `agy` among `command_lines` (a process's parent first, then
/// its ancestors; `None` for one that could not be read) decides; with
/// none of them `agy`, true.
pub fn run_is_headless(command_lines: &[Option<Vec<String>>]) -> bool;
```

- [ ] **Step 1: Write the failing tests** in `antigravity.rs`'s test module:

```rust
fn argv(s: &str) -> Vec<String> { s.split_whitespace().map(str::to_string).collect() }

#[test]
fn agy_is_recognised_by_its_program_name() {
    assert!(is_agy(&argv("agy")));
    assert!(is_agy(&argv("/Users/x/.local/bin/agy --model m")));
    assert!(is_agy(&argv("/bin/bash /tmp/t/agy -p x")));
    assert!(!is_agy(&argv("node /x/agy.js")));
    assert!(!is_agy(&argv("bash ./scripts/ensure-clax.sh exec hook --agent antigravity stop")));
    assert!(!is_agy(&[]));
}
#[test]
fn print_flags_mean_headless() {
    for c in ["agy -p hi", "agy --print hi", "agy --prompt hi", "agy --print=hi", "agy --model m -p hi", "/bin/bash /t/agy -p hi"] {
        assert!(headless(&argv(c)), "{c}");
    }
    for c in ["agy", "agy --model m", "agy -c", "agy --conversation abc", "/bin/bash /t/agy"] {
        assert!(!headless(&argv(c)), "{c}");
    }
}
#[test]
fn an_interactive_prompt_that_mentions_p_is_interactive() {
    assert!(!headless(&argv("agy -i explain the -p flag")));
    assert!(!headless(&argv("agy --prompt-interactive use --print here")));
    assert!(headless(&argv("agy -p explain the -i flag")));
}
#[test]
fn the_nearest_agy_decides_and_unknown_is_headless() {
    let l = |s: &str| Some(argv(s));
    assert!(!run_is_headless(&[l("agy")]));
    assert!(run_is_headless(&[l("agy -p x")]));
    assert!(!run_is_headless(&[l("sh -c ./scripts/tool-hook.sh"), l("agy")]));
    assert!(!run_is_headless(&[None, l("agy")]));
    assert!(run_is_headless(&[None]));
    assert!(run_is_headless(&[]));
    assert!(run_is_headless(&[l("zsh")]));
}
```

- [ ] **Step 2: Run** `just test-affected`. Expected: the four tests fail to compile (`antigravity` module missing).
- [ ] **Step 3: Implement** the constants and the three functions. They read no process: callers pass what `process_argv` gave them.
- [ ] **Step 4: Run** `just test-affected`. Expected: pass.
- [ ] **Step 5: Commit** `feat(core): Antigravity's names and its headless check`.

### Task 2: Hook input and output in Antigravity's shapes

**Files:**
- Modify: `crates/clax-hooks/src/input.rs`, `crates/clax-hooks/src/output.rs`

**Interfaces:**
- Produces:

```rust
impl HookInput {
    /// Antigravity's envelope: it carries `conversationId`.
    pub fn is_antigravity_envelope(&self) -> bool;
    /// Unchanged name; now false for Antigravity's envelope, which also
    /// carries `hookEventName`.
    pub fn is_grok_envelope(&self) -> bool;
    /// `terminationReason` of Antigravity's Stop.
    pub fn termination_reason(&self) -> Option<&str>;
}
impl HookOutput {
    /// This output as Antigravity reads it: a block becomes
    /// `{"decision":"continue","reason":…}`, added context becomes
    /// `{"injectSteps":[{"systemMessage":{"systemMessage":…}}]}`, and
    /// anything else prints nothing.
    pub fn for_antigravity(self) -> HookOutput;
}
```

`HookInput::parse` also fills `session_id` from `conversationId` (after `sessionId`) and sets `stop_hook_active = Some(true)` when neither spelling is present and `executionNum` is above 0.

- [ ] **Step 1: Write the failing tests:**

```rust
// input.rs
#[test]
fn antigravity_keys_fill_the_known_fields() {
    let i = HookInput::parse(r#"{"conversationId":"c1","hookEventName":"Stop","cwd":"/w","transcriptPath":"/t.jsonl","executionNum":1,"terminationReason":"NO_TOOL_CALL"}"#);
    assert_eq!(i.session_id.as_deref(), Some("c1"));
    assert_eq!(i.transcript_path.as_deref(), Some("/t.jsonl"));
    assert_eq!(i.stop_hook_active, Some(true));
    assert_eq!(i.termination_reason(), Some("NO_TOOL_CALL"));
    assert!(i.is_antigravity_envelope());
    assert!(!i.is_grok_envelope());
}
#[test]
fn a_first_stop_is_not_stop_hook_active() {
    let i = HookInput::parse(r#"{"conversationId":"c1","hookEventName":"Stop","executionNum":0}"#);
    assert_ne!(i.stop_hook_active, Some(true));
}
// output.rs
#[test]
fn a_block_becomes_a_continue() {
    assert_eq!(HookOutput::block("r").for_antigravity().value(),
        Some(&json!({"decision": "continue", "reason": "r"})));
}
#[test]
fn added_context_becomes_an_injected_system_message() {
    assert_eq!(HookOutput::additional_context("UserPromptSubmit", "t").for_antigravity().value(),
        Some(&json!({"injectSteps": [{"systemMessage": {"systemMessage": "t"}}]})));
}
#[test]
fn nothing_else_is_printed_for_antigravity() {
    assert_eq!(HookOutput::none().for_antigravity().value(), None);
    assert_eq!(HookOutput::deny("x").for_antigravity().value(), None);
}
```

The existing `grok_camel_case_keys_fill_the_known_fields` and `claude_and_codex_input_is_not_a_grok_envelope` must still pass.

- [ ] **Step 2: Run** `just test-affected`. Expected: the new tests fail to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `just test-affected`. Expected: pass.
- [ ] **Step 5: Commit** `feat(hooks): read and answer Antigravity's hook shapes`.

### Task 3: The hook events under `--agent antigravity`

**Files:**
- Modify: `crates/clax-hooks/src/events.rs` (`prompt`, `stop_with`, `unwoken`), `crates/clax-commands/src/commands/hook.rs` (`Agent`, `handle`, `run`), `scripts/tool-hook.sh` and its copies in `plugins/claude-code/scripts/` and `plugins/clax/scripts/`
- Test: `crates/clax-hooks/tests/golden.rs` and `crates/clax-hooks/tests/fixtures/antigravity-*.json`, `scripts/test-tool-hook.sh`

**Interfaces:**
- Consumes: Task 1's `run_is_headless`; Task 2's `for_antigravity`, `termination_reason`; `process_args` and `ancestors` in `hook.rs`.
- Produces: `clax hook --agent antigravity <stop|prompt|tool>`; in `events.rs`

```rust
/// [`prompt`], reporting the prompt as the person's only when `typed`.
pub fn prompt_with(harness: &str, input: &HookInput, daemon: &dyn Daemon, typed: bool) -> anyhow::Result<HookOutput>;
```

and `tool-hook.sh antigravity` (PostToolUse) and `tool-hook.sh antigravity prompt` (PreInvocation).

Behaviour:
- `prompt_with(.., false)` takes `tier=prompt_hook` with no `prompt=true`. `prompt` keeps calling it with `true`.
- `stop_with` returns nothing for `antigravity` when `termination_reason()` is present and not `NO_TOOL_CALL`.
- `unwoken` applies to `antigravity`; when `lost`, its first sentence is `Your earlier wait_for_feedback call has ended. ` for this harness.
- `handle`: for `Agent::Antigravity`, `SessionStart`, `SessionEnd`, `CallId`, `Ask` and `Asked` return `HookOutput::none()` before any daemon is looked for; `Prompt` calls `prompt_with(.., false)`; `Stop` passes a `StopWake` only when `!run_is_headless(..)` over the command lines (`process_args`) of the parent and of `ancestors(parent_pid)`.
- `run`: an Antigravity run prints `out.for_antigravity()`.
- `tool-hook.sh`: accepts `antigravity`; for it the session ID is read from `"conversationId"`. With a second argument `prompt` it exits 0 unless the input's `invocationNum` is 0 or absent, then runs `ensure-clax.sh exec hook --agent antigravity prompt` with the input on stdin and the hook's stdout passed through (the `tool` mode keeps discarding it).

- [ ] **Step 1: Add fixtures** from spec Appendix A: `antigravity-pre-invocation.json` (`invocationNum` 0), `antigravity-stop.json` (`executionNum` 0, `NO_TOOL_CALL`), `antigravity-stop-continued.json` (`executionNum` 1), `antigravity-stop-canceled.json` (`terminationReason` `USER_CANCELED`), `antigravity-post-tool-use.json` (name `mcp_clax-antigravity_clax_publish`). Each has `conversationId` `c0ffee00-0000-4000-8000-000000000001`.
- [ ] **Step 2: Write the failing golden tests** in `golden.rs`, beside Grok's. Where a test needs an interactive run, the hook is started by a fake `agy` (`clax_fake_exe::install(&dir.join("agy"), …)`, a bash script that runs `"$CLAX_TEST_BIN" hook --agent antigravity "$1"` as its child, as `crates/clax-mcp/tests/channel.rs` fakes `claude`); started directly, the run counts as headless:
  - `antigravity_prompt_injects_pending_feedback_as_a_system_message`: with one comment waiting for the session, stdout is `{"injectSteps":[{"systemMessage":{"systemMessage":<the feedback text>}}]}`, and the daemon's request had no `prompt=true`.
  - `antigravity_stop_continues_the_turn_with_feedback`: stdout is `{"decision":"continue","reason":<text>}`.
  - `antigravity_stop_after_a_continue_takes_no_resends`: with `antigravity-stop-continued.json` the take is `tier=stop_hook&resends=false`.
  - `antigravity_stop_on_another_termination_does_nothing`: `antigravity-stop-canceled.json` prints nothing and makes no take.
  - `antigravity_stop_reminds_once_to_open_a_wait`: under the fake `agy`, a session watching with replies on and no open wait gets one continue whose reason contains `Call wait_for_feedback with no timeout_s now`; a second stop prints nothing.
  - `antigravity_stop_gives_no_reminder_in_a_headless_run`: the same session, with the hook started directly or under the fake `agy -p x`, prints nothing.
  - `antigravity_hooks_do_nothing_for_an_unknown_conversation`: `stop`, `prompt` and `tool` for a conversation ID with no live session print nothing and exit 0.
  - `antigravity_session_start_and_end_do_nothing`.
  - Extend `no_daemon_prints_nothing_and_starts_none` to `antigravity`.
- [ ] **Step 3: Write the failing shell tests** in `scripts/test-tool-hook.sh`: `antigravity prompt` with `"invocationNum":0` runs the hook and passes its stdout through; with `"invocationNum":2` it runs nothing; with the key absent it runs the hook; `antigravity` (tool mode) stamps `run/tool-hook/antigravity-<ID>` and runs at most once a minute; a `conversationId` of `../x` stamps nothing.
- [ ] **Step 4: Run** `just test-affected`. Expected: the new tests fail (`antigravity` is not an agent).
- [ ] **Step 5: Implement**, then copy `scripts/tool-hook.sh` over its two plugin copies.
- [ ] **Step 6: Run** `just test-affected`. Expected: pass, Grok's and Claude Code's golden tests unchanged.
- [ ] **Step 7: Commit** `feat(hooks): Antigravity's stop, turn-start and tool hooks`.

### Task 4: The daemon knows the harness

**Files:**
- Modify: `crates/clax-server/src/routes/sessions.rs` (`HARNESSES`, `Live`, `push_info`, `wakes_when_idle`), `crates/clax-store/src/feedback.rs` (`waiting_on`), `crates/clax-http/src/surface.rs` (`INFERRED`, `note_poll`), `crates/clax-core/src/wake.rs` (`block`), `crates/clax-store/src/toolpath/mod.rs` (display names), and every user of `HARNESSES`' length (`crates/clax-server/src/linked.rs`, `routes/watches.rs`)

**Interfaces:**
- Consumes: Task 1's `HARNESS`.
- Produces: `Live { .., pub waiting: bool }` (`waiters.is_waiting(sid)`); for an `antigravity` session `push` is `{"tier":"wait","available":<waiting>,"reason":<null or the text below>}`; `wake::block("antigravity", ..)` as below. A `push` object may carry `"headless": true` (Task 6 sets it).

Exact texts:
- `push.reason` with no open wait: `no wait_for_feedback call is open for this conversation; the clax skill opens one after a publish. Meanwhile comments arrive at the end of a turn (Stop hook), at the start of the next turn, or on the next clax tool call`
- `block`, available (`tier` `wait`, `via` `"wait"`): `Start nothing: a comment sent to you wakes this session by finishing your open wait_for_feedback call. End your turn when you are done.`
- `block`, unavailable: `A comment will not wake this conversation while it is idle. Call wait_for_feedback with no timeout_s now; it runs in the background. Keep exactly one open, then end your turn.`
- `block`, unavailable and `push["headless"] == true`: `Nothing wakes a headless run. Comments reach you on your next Clax tool call. End your turn when you are done.`

- [ ] **Step 1: Write the failing tests:**
  - `wake.rs`: `antigravity_with_an_open_wait_starts_nothing` (`via == "wait"`, `when_idle == true`), `antigravity_without_a_wait_is_told_to_open_one`, `a_headless_antigravity_run_is_told_nothing_wakes_it`; each asserts the `do` text above and that no `follow_command` key is present.
  - `routes/sessions.rs`: `push_info` and `wakes_when_idle` for `antigravity` with `waiting` true and false; `check_harness("antigravity")` passes.
  - `feedback.rs`: `waiting_on("antigravity", true, true, false) == Tier::StopHook` and `Tier::Piggyback` when not armed.
  - `surface.rs` (or its store tests): for an `antigravity` session, a `prompt_hook` poll after a turn in which nothing came from Clax records terminal activity; the same poll after a `wait` handover does not; a `wait` poll records nothing.
- [ ] **Step 2: Run** `just test-affected`. Expected: fail.
- [ ] **Step 3: Implement.** `INFERRED` becomes `["codex", "grok", "antigravity"]`; `note_poll` returns early for `Tier::Wait` when the session's harness is `antigravity`, as it does for `Tier::Queue`. `waiting_on` adds `"antigravity"` to the `"claude" | "codex" | "grok"` arm. Display name `Antigravity`, company `google`.
- [ ] **Step 4: Run** `just test-affected`. Expected: pass.
- [ ] **Step 5: Commit** `feat(daemon): the antigravity harness, its push state and wake block`.

### Task 5: A session per conversation in the shim

**Files:**
- Create: `crates/clax-mcp/src/conversations.rs`
- Modify: `crates/clax-mcp/src/lib.rs`, `crates/clax-mcp/src/shim.rs` (`Harness`, `registration`, `run`, `serve`), `crates/clax-mcp/src/tools.rs` (`ClaxTools`), `crates/clax-commands/src/commands/mcp.rs` (`Agent`)
- Test: `crates/clax-mcp/tests/shim.rs`

**Interfaces:**
- Consumes: Task 1's `META_ROOT`, `META_CONVERSATION`, `HARNESS`.
- Produces:

```rust
// shim.rs
pub enum Harness { Claude, Codex, Grok, Antigravity }   // as_str: "antigravity"

// conversations.rs
/// Who made a tool call, from its `_meta`.
pub struct Caller { pub root: String, pub subagent: bool }
/// `None` when neither key holds a non-empty string. `root` is META_ROOT,
/// else META_CONVERSATION; `subagent` when both are present and differ.
pub fn caller(meta: &rmcp::model::RequestMetaObject) -> Option<Caller>;

/// The tools of one `agy` process: a Clax session per root conversation.
pub struct Conversations { /* private */ }
impl Conversations {
    pub fn new(template: ClaxTools, base: RegisterSession,
               connect: Arc<dyn Fn(RegisterSession) -> DaemonClient + Send + Sync>,
               heartbeat: Duration) -> Conversations;
    /// The tools of `root`'s session (of the session with no harness
    /// session ID for `None`), made and heartbeated on first use.
    pub fn tools_for(&self, root: Option<&str>) -> ClaxTools;
    /// Settles reports and ends every session, each within `within`;
    /// returns the root conversation IDs it held.
    pub async fn end_all(&self, within: Duration) -> Vec<String>;
}
impl rmcp::ServerHandler for Conversations { /* get_info, list_tools and
    supported_protocol_versions from the template; call_tool routes */ }

// tools.rs
impl ClaxTools {
    /// These tools on `client`, with their own per-session state.
    pub fn for_client(&self, client: DaemonClient) -> ClaxTools;
    /// These tools for a subagent: no feedback is appended to results and
    /// `wait_for_feedback` is refused.
    pub fn without_feedback(&self) -> ClaxTools;
}
```

Behaviour:
- `registration` for `Harness::Antigravity`: no ID from the environment; `cwd` is the parent's (as Codex's). `run` for it registers nothing before serving and does not start a heartbeat until a conversation exists.
- `Conversations::call_tool`: `caller(&context.meta)` (falling back to `request.meta`); `tools_for(Some(root))` or `tools_for(None)`; `.without_feedback()` for a subagent; then that `ClaxTools`'s own `call_tool`.
- Each conversation's client is `connect(RegisterSession { harness_session_id: Some(root), ..base })`, a managed client, so registration, refresh and re-registration are the existing code. The `None` conversation's registration has no harness session ID.
- A subagent's refused wait is an error result with the text `wait_for_feedback is for the main conversation: call it there, not from a subagent.`
- On exit `run` calls `end_all(END_TIMEOUT)` and, for each returned ID matching `[A-Za-z0-9._-]+`, removes `<home>/run/tool-hook/antigravity-<ID>` and `<home>/run/wake-nudge/antigravity-<ID>.json`.
- `commands/mcp.rs`: `Agent::Antigravity` maps to `Harness::Antigravity`; no channel.

- [ ] **Step 1: Write the failing tests** in `tests/shim.rs`, with a `Shim::start_antigravity` beside `Shim::start_grok` and a helper that calls a tool with given `_meta`:
  - `an_antigravity_shim_registers_no_session_before_a_call`: after `tools/list`, the daemon lists no `antigravity` session.
  - `two_root_conversations_get_two_sessions`: `status` with roots `r1` and `r2` gives two live sessions whose `harness_session_id`s are `r1` and `r2`.
  - `a_subagent_acts_in_the_root_session_and_is_handed_nothing`: with a comment waiting for `r1`'s session, a call with `conversation_id` `s1`, `root_conversation_id` `r1` creates no third session and its result has no feedback; the next root call's result has the comment.
  - `a_subagent_cannot_wait`: its `wait_for_feedback` is the error above.
  - `a_call_with_no_conversation_registers_without_a_harness_session_id`.
  - `empty_or_non_string_conversation_keys_count_as_absent`: `_meta` with `""` and with a number for both keys behaves as the previous test.
  - `every_conversation_is_ended_on_exit_and_its_hook_files_removed`: both sessions are ended after stdin closes; a stamp and a ledger file planted for `r1` are gone; a file planted beside them for the root `..` is untouched.
  - `conversations::caller` unit tests for the four key combinations.
- [ ] **Step 2: Run** `just test-affected`. Expected: fail to compile.
- [ ] **Step 3: Implement.** Extract the heartbeat loop in `run` into a function both paths use; make `serve` generic over the handler.
- [ ] **Step 4: Run** `just test-affected`. Expected: pass, the other harnesses' shim tests unchanged.
- [ ] **Step 5: Commit** `feat(mcp): the Antigravity shim keeps a session per conversation`.

### Task 6: The unbounded wait

**Files:**
- Modify: `crates/clax-mcp/src/tools.rs` (`ClaxTools`, `do_wait`, `refined_push`), `crates/clax-mcp/src/shim.rs` (`run`)
- Test: `crates/clax-mcp/tests/shim.rs`

**Interfaces:**
- Consumes: Task 1's `run_is_headless`; `crate::channel::process_argv`; Task 4's wake block and `"headless"`; Task 5's `Conversations`.
- Produces:

```rust
impl ClaxTools {
    /// `wait_for_feedback` with no `timeout_s` waits until there is
    /// something to hand over (an interactive Antigravity run).
    pub fn with_unbounded_wait(self, on: bool) -> ClaxTools;
    /// `status` and `watch` say nothing wakes this run.
    pub fn headless(self, on: bool) -> ClaxTools;
}
```

Behaviour:
- `run` for Antigravity: `headless = run_is_headless(&[process_argv(parent_pid).ok()])`; the template gets `.with_unbounded_wait(!headless).headless(headless)`.
- `do_wait`, unbounded and `timeout_s` absent: repeat `client.feedback("wait", MAX_WAIT_S, artifact)` until the answer has feedback, an answer or a withheld notice, then return it as today. A daemon error is returned as today.
- One open unbounded wait per `ClaxTools`: a second call while one is open returns at once `{"already_waiting": true, "call_again": false}`. The mark clears when the first returns or its future is dropped (the client cancelled).
- With `timeout_s`, and in a headless run, `do_wait` is unchanged.
- `refined_push` for an `antigravity` session sets `push["headless"] = true` in a headless run.

- [ ] **Step 1: Write the failing tests.** The shim is started by a fake `agy` script, as `tests/channel.rs` starts it by a fake `claude`: `agy` for an interactive run, `agy -p x` for a headless one. The daemon fixture's debug `FeedbackWaiters::expire` ends a poll at once, so no test waits:
  - `an_unbounded_wait_outlasts_a_server_poll`: after one expired poll the call is still open, and the daemon has received a second `tier=wait` poll.
  - `an_unbounded_wait_returns_the_comment`: a comment sent to the session completes the call with it, and `call_again` is false.
  - `a_second_wait_while_one_is_open_returns_at_once`: `already_waiting` true, and only one poll is open.
  - `a_cancelled_wait_can_be_opened_again`: after `notifications/cancelled` for the first call, a new call opens a poll.
  - `a_wait_with_a_timeout_is_bounded`: `timeout_s: 5` with an expired poll returns `call_again: true`.
  - `a_headless_run_waits_with_bounds_and_says_nothing_wakes_it`: with the shim started headless, a wait with no `timeout_s` returns `call_again: true` after one expired poll, and `status`'s `wake.do` is the headless text of Task 4.
- [ ] **Step 2: Run** `just test-affected`. Expected: fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `just test-affected`. Expected: pass.
- [ ] **Step 5: Commit** `feat(mcp): wait_for_feedback has no cap in an interactive Antigravity run`.

### Task 7: The plugin, embedded and checked

**Files:**
- Create: `plugins/clax-antigravity/.antigravity-plugin/plugin.json`, `mcp_config.json`, `hooks.json`, `rules/AGENTS.md`, `skills/clax/SKILL.md`, `scripts/ensure-clax.sh`, `scripts/tool-hook.sh`, `README.md`
- Modify: `crates/clax-embedded/build.rs` (`PLUGIN_DIRS`), `crates/clax-embedded/src/plugins.rs`, `crates/clax-commands/src/plugins.rs` (tests), `crates/clax-mcp/src/plugin.rs` (`MANIFESTS`), `plugins/BUILD.bazel`, `crates/clax-server/src/routes/guards.rs` (the two plugin lists in its tests), `scripts/test-plugins.sh`, `scripts/sync-skill-tools.py`, `scripts/bump-version.sh`, `scripts/check-version.sh`, `scripts/pin-release.sh`, `scripts/test-release.files`, `scripts/test-release.sh`

**Interfaces:**
- Consumes: Tasks 3, 5 and 6 (the commands the files run).
- Produces: the plugin tree under `plugins/clax-antigravity/`, written by `clax init` to `~/.clax/marketplace/plugins/clax-antigravity/`.

Exact contents:
- `plugin.json`: `name` `clax-antigravity`, `version` the workspace's, `description`, `author`, `keywords` as `plugins/clax-grok/.grok-plugin/plugin.json` has them, with "Antigravity" for "Grok Build".
- `mcp_config.json` and `hooks.json`: as spec 6.1 prints them. The eleven eager tools are `wait_for_feedback` (also `"background": "always"`), `publish`, `read`, `list`, `open`, `status`, `watch`, `working`, `comments_read`, `comments_reply`, `comments_resolve`.
- `rules/AGENTS.md`: no frontmatter, under 1,500 bytes, covering the five points of spec 6.6.
- `skills/clax/SKILL.md`: `plugins/clax-grok/skills/clax/SKILL.md` as the base, with the shared sections unchanged, the intro rewritten for the two tool-name forms (spec 6.6), and "Live feedback in Grok" replaced by "Live feedback in Antigravity" (spec 6.4 and 6.6).
- `README.md`: `plugins/clax-grok/README.md` as the base: install, what it adds, the permission rule (spec 6.7), feedback tiers (spec 7), headless runs, the Claude Code copy (spec 6.8), troubleshooting.

- [ ] **Step 1: Write the failing checks** in `scripts/test-plugins.sh`, beside Grok's: the required files exist and parse; manifest `name` is `clax-antigravity`; `mcp_config.json` has the single server `clax` with args `["${PLUGIN_ROOT}/scripts/ensure-clax.sh","exec","mcp","--agent","antigravity"]`, `timeoutSeconds` `-1`, `wait_for_feedback` background and eager, exactly the eleven eager tools, each a tool of `plugins/pi/test/fixtures/contract.json`; `hooks.json` has exactly `PreInvocation`, `Stop` and `PostToolUse` in one group and every command starts with `./scripts/`; the wrapper and `tool-hook.sh` equal their root copies; the skill has the shared sections and a `## Live feedback in Antigravity` heading; when `agy` is on `PATH`, `agy plugin validate --strict plugins/clax-antigravity` exits 0. Add the plugin to the embedded-file list test in `crates/clax-embedded/src/plugins.rs` and to `crates/clax-commands/src/plugins.rs`'s materialize test (`scripts/*.sh` are 0755).
- [ ] **Step 2: Run** `just test-affected`. Expected: fail (no such plugin).
- [ ] **Step 3: Write the plugin files**, add the embedding and list entries, and run `scripts/sync-skill-tools.py` to generate the skill's tool block.
- [ ] **Step 4: Run** `just test-affected`. Expected: pass. If `agy plugin validate --strict` asks for a logo, add `assets/logo.svg` (a copy of the mark the web UI uses) and `"logo"` to the manifest.
- [ ] **Step 5: Commit** `feat(plugins): the Antigravity plugin`.

### Task 8: The Claude Code copy stands down under Antigravity

**Files:**
- Modify: `crates/clax-commands/src/host.rs`, `crates/clax-commands/src/commands/hook.rs`, `crates/clax-commands/src/commands/mcp.rs`, `crates/clax-mcp/src/standdown.rs`, `scripts/ensure-clax.sh` and its four plugin copies
- Test: `crates/clax-cli/tests/standdown.rs`, `scripts/test-ensure-clax.sh`, `scripts/test-plugins.sh`

**Interfaces:**
- Consumes: Task 1's `is_agy`; `clax_mcp::channel::process_argv`; Task 2's `is_antigravity_envelope`.
- Produces:

```rust
// host.rs
pub fn antigravity_runs_hook(input: &HookInput) -> bool;            // input.is_antigravity_envelope()
pub fn antigravity_runs_mcp(parent_argv: Option<&[String]>) -> bool;   // is_agy
/// `host` is "grok" or "antigravity".
pub fn log_standdown(home: &Home, mode: &str, host: &str);
// standdown.rs
pub const ANTIGRAVITY_STANDDOWN: &str;
```

`ANTIGRAVITY_STANDDOWN`: `Clax runs from the clax-antigravity plugin in Antigravity, and this copy (the Claude Code plugin, imported into Antigravity) does nothing here. If the Clax tools are missing, run: clax init --agent antigravity. To remove this copy: agy plugin uninstall clax`

- [ ] **Step 1: Write the failing tests:**
  - `standdown.rs`: `the_claude_code_copy_antigravity_starts_serves_only_status` (started by a fake `agy` script; `status` returns `ANTIGRAVITY_STANDDOWN`); `a_claude_code_hook_antigravity_runs_does_nothing_and_logs_its_host` (stdin an Antigravity envelope; nothing printed; hooks.log has `standdown mode=hook agent=claude host=antigravity`); `a_claude_code_hook_grok_runs_still_logs_grok`; `claude_code_itself_still_acts_with_antigravitys_variables_inherited` (`ANTIGRAVITY_CONVERSATION_ID` set, Claude Code's own envelope).
  - `scripts/test-ensure-clax.sh`: the wrapper's own check, in both modes, with the same cases.
  - `scripts/test-plugins.sh`: the wrapper's stand-down text equals `standdown.rs`'s.
- [ ] **Step 2: Run** `just test-affected`. Expected: fail.
- [ ] **Step 3: Implement.** In the wrapper, which must not read stdin, hook mode stands down for `--agent claude` when `ANTIGRAVITY_CONVERSATION_ID` is set and `CLAUDE_PLUGIN_ROOT` is not (Antigravity sets the first for its hooks; Claude Code sets the second for its own); mcp mode when the first or second word of `ps -o args= -p $PPID` has the file name `agy`. `host=` in every stand-down log line comes from which check matched.
- [ ] **Step 4: Run** `just test-affected`. Expected: pass, Grok's stand-down tests unchanged.
- [ ] **Step 5: Commit** `feat: the Claude Code copy stands down under Antigravity`.

### Task 9: `clax init`, `clax uninit` and the permission offer

**Files:**
- Create: `crates/clax-commands/src/antigravity_permissions.rs`
- Modify: `crates/clax-commands/src/lib.rs`, `crates/clax-commands/src/commands/init.rs`
- Test: `crates/clax-cli/tests/init.rs`

**Interfaces:**
- Consumes: Task 1's `PLUGIN`, `PERMISSION_RULE`, `SERVER`; Task 7's embedded plugin.
- Produces:

```rust
pub const SETUP_COMMAND: &str = "clax init --agent antigravity";
/// `<home dir>/.gemini/antigravity-cli/settings.json`.
pub fn settings_path(home_dir: &Path) -> PathBuf;
pub enum State {
    /// `permissions.allow` has the rule (or `mcp(*)`).
    Allowed,
    Missing,
    /// `ask` or `deny` rules that name this server: `mcp(*)`,
    /// `mcp(clax-antigravity_clax/*)` or `mcp(clax-antigravity_clax/<tool>)`.
    Blocked(Vec<String>),
}
/// Of the settings file's text; `Err` when it is not a JSON object or
/// `permissions`, `allow`, `ask` or `deny` has the wrong type.
pub fn assess(text: &str) -> Result<State, String>;
/// The text with the rule appended to `permissions.allow`, every other
/// key kept; an empty or missing file becomes `{"permissions":{"allow":[…]}}`.
pub fn with_rule(text: &str) -> Result<String, String>;
```

`init.rs`: a `HARNESSES` entry `antigravity` between `grok` and `pi`: removal `agy plugin uninstall clax-antigravity` (not required), addition `agy plugin install <root>/plugins/clax-antigravity` (required), `record` `{"plugin": "clax-antigravity", "source": <dir>}`, `uses` true when `agy plugin list --json` lists a plugin named `clax-antigravity` (or, without `agy` on `PATH`, when `registrations.json` records one under `root`), `by_hand` `agy plugin uninstall clax-antigravity`. After a successful install the offer runs, in the way `codex_settings` (`init.rs:800-911`) does: it prints the file, the rule and that the rule approves every Clax tool including `delete`; `--yes` adds, a terminal is asked `[y/N]`, anything else adds nothing. Its JSON status is one of `added`, `unchanged`, `declined`, `not_added`, `blocked`, `failed`. `uninit` leaves the rule and says so.

- [ ] **Step 1: Write the failing tests:**
  - Unit, `antigravity_permissions.rs`: `assess` for a file with the rule, with `mcp(*)`, without, with `"ask": ["mcp(clax-antigravity_clax/delete)"]` (`Blocked` naming it), `Missing` for `""`, and `Err` for `"[]"`, `"{"` and `{"permissions":{"allow":"x"}}`; `with_rule` on `""`, on `{"model":"m","trustedWorkspaces":["/w"]}` (both keys kept, rule added), on a file whose `allow` has other rules (appended, not replaced), and twice (the second leaves one rule).
  - `tests/init.rs`, with a fake `agy`: `a_machine_with_only_agy_registers_clax_antigravity` (the recorded commands are the uninstall then the install of `<marketplace>/plugins/clax-antigravity`); `init_yes_adds_the_antigravity_rule_and_keeps_the_other_settings`; `init_without_a_terminal_adds_nothing_and_prints_the_rule`; `init_adds_nothing_beside_an_ask_rule_and_names_it`; `a_settings_file_that_is_not_json_is_left_untouched` (status `failed`, bytes unchanged); `agy_plugin_list_that_is_not_json_counts_as_not_installed`; `uninit_removes_the_plugin_and_leaves_the_rule`.
- [ ] **Step 2: Run** `just test-affected`. Expected: fail.
- [ ] **Step 3: Implement.** `serde_json` in this workspace does not keep key order; write the file pretty-printed with two spaces, as `agy` does, and replace it atomically as `codex_approvals::write_config` does.
- [ ] **Step 4: Run** `just test-affected`. Expected: pass.
- [ ] **Step 5: Commit** `feat(init): register the Antigravity plugin and offer its permission rule`.

### Task 10: `clax doctor --agent antigravity`

**Files:**
- Modify: `crates/clax-commands/src/commands/doctor_agent.rs`
- Test: the doctor tests beside it and in `crates/clax-cli/tests/`

**Interfaces:**
- Consumes: Task 1's `MIN_AGY`, `PLUGIN`; Task 9's `assess`, `settings_path`, `SETUP_COMMAND`.
- Produces: `DoctorAgent::Antigravity` (`--agent antigravity`), running `binary`, `upgrade`, `plugin`, `skill`, `mcp`, `hooks`, `feedback`, then:
  - `antigravity`: `agy --version` parsed and at least 1.5.18; a warning naming both versions when older; not found when `agy` is not on `PATH`.
  - `antigravity_permissions`: ok when `Allowed`; otherwise the rule, the file, `clax init --agent antigravity`, and that headless runs refuse the tools without it; `Blocked` names the rules.
  - `antigravity_sessions`: a warning for each live `antigravity` session with a `pid` and no harness session ID.
  - `claude_copy`: the count of hooks.log lines with `standdown` and `host=antigravity`, with `agy plugin uninstall clax`.

  `plugin` reads `<home dir>/.gemini/config/plugins/clax-antigravity`; `Dirs` gains that root.

- [ ] **Step 1: Write the failing tests**, one per new check and state (version older, equal, unreadable; rule present, missing, blocked; a session with and without the ID; zero and two stand-down lines), plus `the_installed_plugin_is_found_under_gemini_config`.
- [ ] **Step 2: Run** `just test-affected`. Expected: fail.
- [ ] **Step 3: Implement**, following Grok's arms in `checks` (`doctor_agent.rs:1208-1269`).
- [ ] **Step 4: Run** `just test-affected`. Expected: pass.
- [ ] **Step 5: Commit** `feat(doctor): check an Antigravity install`.

### Task 11: The agent's shell, development and the docs

**Files:**
- Modify: `crates/clax-commands/src/channel.rs` (`HARNESS_VARIABLES`), `crates/clax-commands/src/commands/remotes.rs` (`SESSION_VARIABLES`), `scripts/probe-harness-env.sh`, `scripts/dev.sh`, `scripts/test-dev.sh`, `justfile` (the `dev` comment), `docs/contract.md`, `docs/usage.md`, `README.md`

**Interfaces:**
- Produces: `("ANTIGRAVITY_AGENT", Some("1"))` in `HARNESS_VARIABLES`; `("ANTIGRAVITY_CONVERSATION_ID", "antigravity")` in `SESSION_VARIABLES`; `just dev antigravity` runs `agy "$@"` with the dev binary and home, as `just dev grok` runs `grok`.

- [ ] **Step 1: Write the failing tests:** `channel.rs`: `ANTIGRAVITY_AGENT=1` is the agent channel and `ANTIGRAVITY_AGENT=0` is not; `remotes.rs`: a session is found from `ANTIGRAVITY_CONVERSATION_ID`; `scripts/test-dev.sh`: `antigravity` is an accepted harness and runs the fake `agy`.
- [ ] **Step 2: Run** `just test-affected`. Expected: fail.
- [ ] **Step 3: Implement.** `probe-harness-env.sh` matches `ANTIGRAVITY*` and shows the values of `ANTIGRAVITY_AGENT` and `ANTIGRAVITY_CONVERSATION_ID` only.
- [ ] **Step 4: Write the docs** (spec 6.11): in `docs/contract.md` the shim's agent list, a row in "Names as the model sees them" (`mcp_clax-antigravity_clax_<tool>` and `call_mcp_tool`), a `### Antigravity` section under "Sessions" (spec 6.2 to 6.5), the Antigravity bullet in "Harness shell environments" (agy 1.5.18, observed 2026-11-26: `ANTIGRAVITY_AGENT=1`, `ANTIGRAVITY_CONVERSATION_ID`, and the names `ANTIGRAVITY_TRAJECTORY_ID`, `ANTIGRAVITY_AGENTAPI_EXE`, `ANTIGRAVITY_APP_DATA_DIR`, `ANTIGRAVITY_CSRF_TOKEN`, `ANTIGRAVITY_LS_ADDRESS`, `ANTIGRAVITY_LS_VERSION`, `ANTIGRAVITY_PROJECT_ID`, `ANTIGRAVITY_SOURCE_METADATA`, which the CLI does not read), an Antigravity column in "Delivery tiers per harness" (spec 7), and `clax init`; in `docs/usage.md` and `README.md` the install, init, doctor and `just dev` lists.
- [ ] **Step 5: Run** `just test-affected` (it runs `scripts/sync-skill-tools.py --check` over the docs' tool lists). Expected: pass.
- [ ] **Step 6: Commit** `docs: Antigravity as the fifth harness`.

### Task 12: Smoke, gates and records

**Files:**
- Create: `scripts/smoke-antigravity.sh`

- [ ] **Step 1: Write `scripts/smoke-antigravity.sh`** on the model of `scripts/smoke-grok.sh`: manual, calls a model, uses a scratch `CLAX_HOME` on a free port and the person's own `~/.gemini` sign-in. It installs the plugin (`clax init --agent antigravity --yes`), runs `agy -p` to publish a page with `--settings` carrying the permission rule, checks the page exists and that a live `antigravity` session has a harness session ID, checks hooks.log has `agent=antigravity event=stop`, uninstalls, and stops the daemon it started by PID. It prints the interactive steps it cannot run: publish, comment in the browser while idle, see the conversation wake and reply, leave a wait open for more than ten minutes, `/exit` and resume; and, for the imported copy, whose environment the spec's section 12 lists as unobserved: `agy plugin install plugins/claude-code`, start a session, confirm hooks.log has `standdown` lines with `host=antigravity` for both `mode=mcp` and `mode=hook`, then `agy plugin uninstall clax`.
- [ ] **Step 2: Run it** and the interactive steps by hand. Expected: every automated check passes; the conversation wakes on a comment after ten idle minutes. Record what was run and seen in the pull request's description.
- [ ] **Step 3: Run** `just check`. Expected: pass, under two minutes on a warm cache. A lane that fails in untouched code is rerun alone and both results reported.
- [ ] **Step 4: Records.** `qualifier diff main`; re-anchor any drifted record this branch touched; record what the next person would want to know and this plan did not fix (at least: working marks lapse while only a subagent uses tools, pinned to the `tool` event's lines; and any behaviour of spec section 12 the smoke could not confirm). Commit the `.qual` changes separately.
- [ ] **Step 5: Commit** `test: a manual smoke for the Antigravity plugin`.

# Working marks across background subagents: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A thread's working mark stays on while the agent that owns it has background subagents doing its work. Three PRs: PR 1 (this plan's tasks) covers Claude Code and Codex; PR 2 (Pi) and PR 3 (Grok Build) follow, each in its own section below.

**Architecture:** No server change. PR 1: Claude Code's Stop hook skips `working/end` while its input lists a running subagent; every skill states the owner/implementer rule, and the Codex skill tells the owner to wait for its subagents. Pi and Grok behave as today until their PRs.

**Tech Stack:** PR 1: Rust (`clax-hooks`), Markdown skills and docs. PR 2: TypeScript (Pi extension, vitest). PR 3: Rust (`clax-commands`), POSIX sh, plugin config.

**Spec:** `docs/superpowers/specs/2026-11-06-working-across-background-subagents-design.md`

## Global Constraints

- Build and test through `just` (Bazel); never `cargo build`/`cargo test`/`pnpm install` in a worktree. While working run `just test-affected`; run `just check` once before merge.
- No `unsafe`. Never create hard links.
- Tests are load-tolerant: no sleeps standing in for a condition, no wall-clock budgets; timing goes through injected clocks or fake timers.
- Never run an unmerged build against `~/.clax` or the daemon on 7480.
- Commit with `git -c commit.gpgsign=false commit`. No AI attribution lines in commit messages.
- Doc comments and commit messages describe the contract, never the conversation. Write "ID" in prose.
- Before editing a file run `qualifier threads <path>`; open threads on PR 1's files: `crates/clax-hooks/src/events.rs` 97e22f80, 134d9b31 (Stop's wake nudge, unaffected).
- A subagent counts only as a Stop `background_tasks` entry with `"type":"subagent"` and `"status":"running"`. Grok's `backgroundTasks` is not read until PR 3.

## Review Focus (PR 1)

1. A session whose only background task is Clax's comment follower (`clax feedback follow`, a `shell` or `monitor` entry) must still end its records at Stop. Task 1 tests a shell-only list.
2. A Stop that blocks with pending feedback must not change: the subagent check runs only on the path that would post `working/end`. Task 1 tests a blocked stop with a running subagent.
3. A Grok Stop that lists a running subagent in `backgroundTasks` must still end the records, as today. Task 1 tests it.

---

# PR 1: Claude Code and Codex

### Task 1: Claude Code's Stop keeps records while a subagent runs

**Files:**
- Modify: `crates/clax-hooks/src/input.rs` (new method on `HookInput`, tests)
- Modify: `crates/clax-hooks/src/events.rs:447-479` (`stop_with`), tests near `stop_ends_the_turn_only_when_it_allows_the_stop` (line ~1109)
- Modify: `docs/contract.md:4760-4769` (the working paragraph and renewal table)

**Interfaces:**
- Produces: `impl HookInput { pub fn has_running_subagent(&self) -> bool }`: reads `rest["background_tasks"]` only; true when the array has an object with `"type":"subagent"` and `"status":"running"`.

- [ ] **Step 1: Write the failing tests**

In `input.rs` tests:

```rust
#[test]
fn a_running_subagent_is_a_running_subagent_entry() {
    let has = |j: &str| HookInput::parse(j).has_running_subagent();
    assert!(has(r#"{"background_tasks":[{"id":"a","type":"subagent","status":"running","agent_type":"general-purpose"}]}"#));
    assert!(!has(r#"{"backgroundTasks":[{"id":"a","type":"subagent","status":"running","agentType":"explore"}]}"#));
    assert!(!has(r#"{"background_tasks":[{"id":"b","type":"shell","status":"running","command":"clax feedback follow"}]}"#));
    assert!(!has(r#"{"background_tasks":[{"id":"a","type":"subagent","status":"completed"}]}"#));
    assert!(!has(r#"{"background_tasks":[]}"#));
    assert!(!has(r#"{"session_id":"s"}"#));
    assert!(!has(r#"{"background_tasks":"x"}"#));
}
```

In `events.rs` tests:

```rust
#[test]
fn stop_keeps_working_records_while_a_subagent_runs() {
    let d = fake(None);
    let i = HookInput::parse(r#"{"session_id":"s1","background_tasks":[{"id":"a","type":"subagent","status":"running"}]}"#);
    assert_eq!(stop("claude", &i, &d).unwrap(), HookOutput::none());
    assert!(d.posts.borrow().is_empty());
}

#[test]
fn stop_ends_working_records_when_only_shells_run() {
    let d = fake(None);
    let i = HookInput::parse(r#"{"session_id":"s1","background_tasks":[{"id":"b","type":"shell","status":"running"}]}"#);
    stop("claude", &i, &d).unwrap();
    assert_eq!(*d.posts.borrow(), ["/api/sessions/S/working/end"]);
}

#[test]
fn a_stop_with_feedback_blocks_whether_or_not_a_subagent_runs() {
    let d = fake(Some("[clax] 1 comment sent to you:\nX"));
    let i = HookInput::parse(r#"{"session_id":"s1","background_tasks":[{"id":"a","type":"subagent","status":"running"}]}"#);
    assert_ne!(stop("claude", &i, &d).unwrap(), HookOutput::none());
    assert!(d.posts.borrow().is_empty());
}
```

Add `grok_stop_ends_working_records_as_before`: harness `"grok"`, input `{"sessionId":"s1","hookEventName":"Stop","reason":"end_turn","backgroundTasks":[{"id":"a","type":"subagent","status":"running"}]}`, asserting the `working/end` post (add a `grok` row to `fake`'s sessions fixture if it has only `claude`).

- [ ] **Step 2: Run to verify they fail**

Run: `just test-affected`
Expected: FAIL: `has_running_subagent` not found.

- [ ] **Step 3: Implement**

Add `has_running_subagent` to `input.rs`, with a doc comment saying it reads Claude Code's `background_tasks` and that Grok Build's `backgroundTasks` is deliberately not read yet. In `stop_with`'s `Some(None)` arm, after the `unwoken` check and before the `working/end` post, return `Ok(HookOutput::none())` when `input.has_running_subagent()`. Update the `stop` doc comment: it ends the session's working records unless a subagent is still running in the background.

- [ ] **Step 4: Run to verify they pass**

Run: `just test-affected`
Expected: PASS.

- [ ] **Step 5: Update `docs/contract.md`**

In the paragraph above the renewal table, "end your turn" becomes "end your turn (in Claude Code, a turn that ends with a subagent still running in the background keeps them; the turn the subagent's report starts ends them)". In the Claude Code row of the table, replace "(a Stop hook that allows the stop ends the turn instead)" with "(a Stop hook that allows the stop ends the turn instead, unless a background subagent is still running)".

- [ ] **Step 6: Commit**

```bash
git add crates/clax-hooks/src/input.rs crates/clax-hooks/src/events.rs docs/contract.md
git -c commit.gpgsign=false commit -m "feat(hooks): a turn that ends with a background subagent running keeps its working marks"
```

### Task 2: Skills state the owner/implementer rule

**Files:**
- Modify: the "Showing that you are working" paragraph of `plugins/claude-code/skills/clax/SKILL.md:380`, `plugins/clax/skills/clax/SKILL.md:383`, `plugins/pi/skills/clax/SKILL.md:378`, `plugins/clax-grok/skills/clax/SKILL.md:377`

- [ ] **Step 1: Add the paragraph to all four skills, at the end of the "Showing that you are working" part of "## Comment loop"**

`scripts/test-plugins.sh` requires "## Comment loop" to be the same in all four skills, so the paragraph is word for word the same and names the harnesses where they differ:

```markdown
Subagents: you own the threads you take. When you hand a thread's work to
subagents, they do not call `working`, `comments_reply`,
`comments_resolve` or `comments_handoff` on it: they report to you, and
you reply once every subagent working on the thread has reported. In
Claude Code your page stays marked working while they run in the
background. Codex does not wake you when a subagent finishes, so in Codex
wait for your subagents (`wait_agent`) before ending a turn that has a
thread in progress.
```

PR 2 and PR 3 extend its second sentence to Pi and Grok Build.

- [ ] **Step 2: Run the skill checks**

Run: `scripts/test-plugins.sh` and `scripts/check-version.sh`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add plugins/*/skills/clax/SKILL.md
git -c commit.gpgsign=false commit -m "docs(skills): the thread's owner speaks for it; its subagents report to the owner"
```

### Task 3: Close the loop

- [ ] **Step 1:** Run `just check`; report any lane failing in untouched code after rerunning it alone.
- [ ] **Step 2:** `qualifier diff main`; check every record this branch added or drifted.
- [ ] **Step 3:** Manual check in Claude Code against a dev home (`just dev`): publish a page, send a comment, have the agent launch a background subagent and end its turn; the page keeps the mark until the agent's reply after the subagent reports.

# PR 2 (later): Pi

Its own branch after PR 1. The packages' events were checked against their source (`pi-subagents` 0.86.0: `runs/background/async-execution.js`, `runs/background/result-watcher.js`; `@tintinweb/pi-subagents` 0.23.1: `src/index.ts` `buildEventData`, the `subagents:started` emit). Both packages need Pi 1.x while the extension's tests run against Pi 0.73.1, so the package events are covered by unit tests against fake events, not end to end.

Review focus: a background completion that arrives while a turn is running must not end the turn's records (the next `agent_end` ends them); a completion for an ID never seen started is ignored; a subagent whose completion never arrives stops keeping the marks after `MAX_SUBAGENT_MS`. Task P1 tests all three.

### Task P1: Pi renews through long tool calls and background subagents

**Files:**
- Create: `plugins/pi/src/keepalive.ts`
- Create: `plugins/pi/test/keepalive.test.ts`
- Modify: `plugins/pi/src/clax.ts:2680-2697` (replace the `tool_call`/`agent_end` renewal with `Keepalive`)
- Modify: `plugins/pi/test/fake-api.ts` (`FakePi` gains an `events` bus: `on(name, handler)` returning an unsubscribe, and `emitBus(name, data)`)
- Modify: `plugins/pi/test/clax.test.ts:1643` (the existing renewal test, adjusted), `docs/contract.md` Pi row, `plugins/*/skills/clax/SKILL.md` (all four, kept identical: the subagents paragraph's second sentence names Pi with `pi-subagents` or `@tintinweb/pi-subagents`)

**Interfaces:**
- Produces, in `keepalive.ts`:

```ts
export interface Timers { setInterval(f: () => void, ms: number): unknown; clearInterval(h: unknown): void }
export class Keepalive {
  constructor(renew: () => void, end: () => void, everyMs: number, timers?: Timers);
  turnStarted(): void;
  turnEnded(): void;            // end() now, unless a subagent is outstanding
  toolStarted(id: string): void;
  toolEnded(id: string): void;
  subagentStarted(id: string): void; // records the start time
  subagentEnded(id: string): void; // an unknown ID is ignored; the last one ends the marks when no turn runs
  stop(): void;                 // clears everything, no end()
}
```

  While a tool call or a subagent is outstanding, an interval calls `renew()` every `everyMs`; with neither, no interval runs. A subagent outstanding longer than `MAX_SUBAGENT_MS` (exported, 60 minutes) is dropped at the next tick as if it had ended, so a completion that never arrives cannot keep the marks on.
- Wiring in `clax.ts` (`RENEW_EVERY_MS` stays 15 s): `agent_start` → `turnStarted`; `agent_end` → `turnEnded`; `tool_execution_start`/`tool_execution_end` → `toolStarted`/`toolEnded` with `event.toolCallId`; `tool_call` keeps its throttled renew; on `pi.events`: `pi-subagents` 0.86.0: `subagent:async-started` → `subagentStarted(data.id)` (its `id` is the run ID), `subagent:async-complete` → `subagentEnded(data.runId ?? data.id)`; `@tintinweb/pi-subagents` 0.23.1: `subagents:started` → `subagentStarted(data.id)` (fires for every top-level agent; a foreground one ends inside its turn, so it changes nothing), `subagents:completed` and `subagents:failed` → `subagentEnded(data.id)`; payloads without a string ID are ignored; `session_shutdown` → `stop()`. `renew`/`end` call `renewWorking`/`endWorking` on `tools.existingClient()` when it has a session, swallowing errors as today.

- [ ] **Step 1: Write the failing tests**

`keepalive.test.ts`, with fake timers (`vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] })`):

- `"a long tool call renews every interval until it ends"`: `toolStarted("t")`, advance 45 s → 3 renews; `toolEnded("t")`, advance 30 s → still 3.
- `"a turn ending with a background subagent keeps the marks until it completes"`: `turnStarted(); subagentStarted("a"); turnEnded()` → 0 ends; advance 30 s → 2 renews; `subagentEnded("a")` → 1 end, interval cleared.
- `"a completion during a turn leaves the marks to that turn's end"`: `subagentStarted("a"); turnStarted(); subagentEnded("a")` → 0 ends; `turnEnded()` → 1 end.
- `"an unknown completion is ignored"`: `turnStarted(); subagentEnded("x"); turnEnded()` → 1 end, 0 renews.
- `"a turn with nothing outstanding ends at once"`: `turnStarted(); turnEnded()` → 1 end.
- `"a subagent whose completion never arrives is dropped after MAX_SUBAGENT_MS"`: `turnStarted(); subagentStarted("a"); turnEnded()`; advance `MAX_SUBAGENT_MS` + 15 s → 1 end, interval cleared.
- `"stop clears without ending"`: `subagentStarted("a"); stop()`; advance 60 s → 0 renews, 0 ends.

`clax.test.ts`: with a session, `pi.emitBus("subagent:async-started", {id: "r1"})`, emit `agent_end` → `endWorking` not called; `pi.emitBus("subagent:async-complete", {runId: "r1"})` → `endWorking` called once. The same with `subagents:started` / `subagents:completed` (`{id: "t1"}`).

- [ ] **Step 2: Run to verify they fail**

Run: `just test-affected`
Expected: FAIL: `keepalive.ts` missing.

- [ ] **Step 3: Implement `Keepalive` and wire it in `clax.ts`**

Replace the comment at :2682 with one describing the new contract (renewed by tool calls, through a tool call still executing and while a background subagent of `pi-subagents` or `@tintinweb/pi-subagents` is outstanding; ended at the end of the agent loop with none outstanding, or when the last completes outside a turn).

- [ ] **Step 4: Run to verify they pass**

Run: `just test-affected`
Expected: PASS.

- [ ] **Step 5: `docs/contract.md` Pi row**

"tool calls (at most every 15 s); a tool call still executing and a background subagent of `pi-subagents` or `@tintinweb/pi-subagents`, every 15 s".

- [ ] **Step 6: Commit**

```bash
git add plugins/pi/src/keepalive.ts plugins/pi/src/clax.ts plugins/pi/test plugins/*/skills/clax/SKILL.md docs/contract.md
git -c commit.gpgsign=false commit -m "feat(pi): working marks last through long tool calls and background subagents"
```


# PR 3 (later): Grok Build

The design and the checks it needs first are in the spec, "Second pass: Grok Build". In short: a live Grok run must confirm how `backgroundTasks` spells a subagent entry, that a child's tool calls fire the plugin's `PostToolUse` with the child's `sessionId`, and that the parent is woken; then the Stop rule reads `backgroundTasks`, the Grok plugin gains `PostToolUse`, `SubagentStart` and `SubagentStop` hooks, and a `run/grok-subagent/<child ID>` alias lets a child's renewals reach its parent. That PR also names Grok Build in the four skills' subagents paragraph, corrects `docs/verification.md:500-504` (Grok has a `PostToolUse` event) and answers qualifier alternative fcca66e6 on `scripts/tool-hook.sh`.

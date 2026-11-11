# Working marks across background subagents

Status: design, 2026-11-06.

## Problem

A working record (`crates/clax-core/src/working.rs`) shows a person that a
harness session is acting on an artifact's threads. The turn-end hook ends
every record of the session (`crates/clax-hooks/src/events.rs:474`, Pi's
`agent_end` in `plugins/pi/src/clax.ts:2691`). When the agent hands a
thread's work to a subagent that runs in the background, the parent's turn
ends while that work goes on, so the page stops showing it, although the
thread is still being worked on and will get a reply.

## Structure: the owner pulls, implementers work

The agent that takes a thread (its owner: `threads.target_session_id`,
`crates/clax-store/src/threads.rs:163`) is the only agent that speaks for it
in Clax. It may hand the work to subagents; those implementers never call
`working`, `comments_reply`, `comments_resolve` or `comments_handoff` on
the owner's thread. They report to the owner, and the owner replies once
every implementer of the thread has reported. Nothing in Clax tracks
implementers by identity: the owner's working record stays on while its
harness reports background subagents running, and each harness's existing
renewal keeps it alive.

## What each harness gives (researched 2026-11-06)

| | Claude Code 2.1.319 | Grok Build (source, `xai-org/grok-build@2bdd1d6a`) | Codex 0.166.0 | Pi |
|---|---|---|---|---|
| Parent turn ends with a subagent running | yes | yes (`task` runs in the background by default) | yes | only with a background-subagent package |
| Turn-end hook lists running subagents | `Stop`: `background_tasks[]` | `Stop`: `backgroundTasks[]` | no | no core signal |
| Subagent tool calls renew the parent | yes: `PostToolUse` carries the parent's `session_id` and an `agent_id` | no: a subagent is its own session (`sessionId` = its `subagentId`) | yes: parent's `session_id`, plus `agent_id` | no: child sessions or processes |
| Parent woken when a subagent finishes | yes (`UserPromptSubmit`, then a turn and a `Stop`) | yes (auto-wake, on by default) | no, unless the under-development `wake_on_agent_messages` feature is on | yes, with the background packages |

Evidence:

- Claude Code: experiment with a hook logging every input, print mode and
  interactive. The parent's `Stop` fired about 0.7 s after launch with
  `"background_tasks":[{"id":…,"type":"subagent","status":"running",…}]`;
  the subagent's `PostToolUse` kept arriving afterwards under the parent's
  `session_id`; on completion the parent took a turn and stopped again with
  `"background_tasks":[]`. `background_tasks` also lists the parent's own
  background shells and monitors, among them Clax's own comment follower
  (`clax feedback follow`).
- Grok Build: `stop_gate.rs:153-226` builds `backgroundTasks` from
  outstanding tasks and active subagents, each subagent as
  `{"type":"subagent","status":"running"}`; `SubagentStart` fires in the
  parent's session with the child's `subagentId`
  (`acp_session_impl/updates.rs:726-745`); a child's tool calls fire
  `PostToolUse` with the child's `sessionId` (`hook_dispatch.rs:253-334`);
  the child's turn end fires `SubagentStop`, never `Stop`. Grok has a
  `PostToolUse` event (`xai-grok-hooks/src/event.rs:69-178`), which
  `docs/verification.md:500-504` says it lacks.
- Codex: experiments over `codex exec` and `codex app-server`, and source at
  tag `rust-v0.166.0`. With default features a finished child's result is
  added to the parent's history without a turn
  (`core/src/agent/control.rs:443-563`,
  `core/src/agent/control/completion.rs:112-118`). A child's Clax calls go
  to a fresh `clax mcp` process: MCP state is per thread
  (`codex-mcp/src/runtime.rs:115`).
- Pi: no built-in subagents (README "No sub-agents… install a package").
  `pi-subagents` 0.86.0 emits `subagent:async-started` and
  `subagent:async-complete` on `pi.events`; `@tintinweb/pi-subagents`
  0.23.1 emits `subagents:started`, `subagents:completed` and
  `subagents:failed` with `id`; a `pi-subagents` completion carries `runId`.
  Both wake the parent with `pi.sendMessage(…, {triggerTurn: true})`. A
  blocking subagent is one long tool call, so the parent's record lapses
  after 120 s, as it does for any tool call longer than that.

## Design

1. **Claude Code: the Stop hook keeps the records while a subagent runs.**
   When the Stop input's `background_tasks` (Claude Code's key only)
   has an entry with `type` `"subagent"` and `status` `"running"`, the hook
   allows the stop without posting `working/end`. Other entry types do not
   count: the comment follower and dev servers are background shells that
   run for the whole session. A subagent waiting on a background shell of
   its own is listed only as that shell, so its parent's record ends early;
   the parent is still woken and replies.
2. **Grok: unchanged in this pass.** Holding the records at Stop without
   a renewal that reaches the parent would only delay the mark's end by
   120 s, and the renewal rests on Grok behaviour read from source, never
   run. Grok's `Stop` keeps ending the records; its `backgroundTasks` key
   is not read. The second pass, and what it must verify first, is under
   "Second pass: Grok Build" below.
3. **Codex: the owner waits.** The Codex skill tells the owner to wait for
   its subagents (`wait_agent`) before ending a turn that has a Clax thread
   in progress. No code changes; the existing Stop behaviour is right, and
   a record that ends at a Stop whose children will never be answered is
   accurate.
4. **Pi (a later PR): renew through long tool calls and packages.** The
   extension renews every `RENEW_EVERY_MS` while a tool call executes
   and while a background subagent of either package is outstanding (at
   most `MAX_SUBAGENT_MS`, 60 minutes, each). At `agent_end` it leaves the
   records alone while any is outstanding, and ends them when the last
   completes outside a turn. Claude Code and Codex land first; Pi and Grok
   behave as today until their PRs. Without either package, no change.
5. **Every skill states the owner/implementer rule.**
6. **No server change.** Records, their 120 s lapse and the page stay as
   they are.

## Not done

- A `background: true` flag on `working`: the hooks of Claude Code and
  Grok already know; Codex never wakes the owner to reply; Pi has events.
- Per-subagent working records, keyed by harness agent IDs: needs a
  different record key and agent cooperation to name each subagent's
  threads; one owner record is what the person needs.
- `SubagentStart`/`SubagentStop` counting for Claude Code: they fire again
  on every pause and resume, and one interactive run had a `SubagentStop`
  with no matching start; `background_tasks` on `Stop` is the harness's
  own answer.
- Codex waking the owner: revisit when `wake_on_agent_messages` is stable;
  it then joins design point 1 if Codex's Stop lists running children.
- Renewing during a single long tool call in Claude Code, Codex or Grok
  (the 120 s lapse during a long Bash): an existing limitation of every
  harness, separate from subagents.

## Second pass: Grok Build

Design, once a live Grok Build confirms it: the Stop rule of design point 1
also reads `backgroundTasks`; the Grok plugin gains a `PostToolUse` hook
(the shared `scripts/tool-hook.sh` gate) and a `SubagentStart` hook that
records the child's ID as an alias of the parent's session ID in
`<CLAX_HOME>/run/grok-subagent/<child ID>`; the tool hook resolves an
aliased `sessionId` to the parent before renewing; `SubagentStop` with
`phase` `"observe"` removes the alias.

Verify first, with a hook that logs every input during a run whose parent
launches a background subagent and ends its turn:

- the Stop input's `backgroundTasks` spells a subagent entry's `type` as
  `"subagent"` (the source builds it from `BackgroundTaskType::Subagent`;
  its serialisation was not found in `xai-org/grok-build@2bdd1d6a`);
- a child's tool calls fire the plugin's `PostToolUse` with the child's
  `sessionId`, and `SubagentStart` carries the parent's `sessionId` and the
  child's `subagentId`;
- the parent is woken when the child finishes;
- Clax's existing Grok Stop-hook block works live (`docs/follow-ups.md`
  lists it as unverified).

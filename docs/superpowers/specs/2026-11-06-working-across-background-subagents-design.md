# Working marks across background subagents

Status: design, 2026-11-06; revised 2026-11-11 to the agent-tracked design.

## Problem

A working record (`crates/clax-core/src/working.rs`) shows a person that a
harness session is acting on an artifact's threads. The turn-end hook ends
every record of the session (`POST /api/sessions/<id>/working/end`, from
`crates/clax-hooks/src/events.rs` and Pi's `agent_end`). When the agent
hands a thread's work to a subagent that runs in the background, the
parent's turn ends while that work goes on, so the page stops showing it,
although the thread is still being worked on and will get a reply.

## Structure: the owner pulls, implementers work

The agent that takes a thread (its owner: `threads.target_session_id`,
`crates/clax-store/src/threads.rs`) is the only agent that speaks for it in
Clax. It may hand the work to subagents; those implementers never call
`working`, `comments_reply`, `comments_resolve` or `comments_handoff` on
the owner's thread. They report to the owner, and the owner replies once
every implementer of the thread has reported.

The owner also says which threads it handed off. Only the owner knows
which thread a subagent works on: a harness reports at most that some
subagent is running (Claude Code's and Grok Build's Stop inputs), never
for which thread, so a rule built on that report keeps every thread of the
session marked, a thread answered in the same turn included.

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

1. **`working` takes `background: true`.** With it, the threads in
   `thread_ids` (required) join the record's threads instead of replacing
   them and are flagged background (`Record::background`). The record's
   view lists them as `background_thread_ids`, absent when none. A later
   call without it replaces only the threads that are not background; a
   background thread it names stays background. A `message` given with
   `background: true` belongs to the background threads, any other to the
   rest of the work. A call that would make the record name more than 20
   threads fails (`invalid_args`) and changes nothing.
2. **A turn end and a publish end only the foreground work.**
   `POST …/working/end` (`End::TurnEnd`) and a publish (`End::Publish`)
   end a live record with background threads as if it held only its
   other threads and their message: `working.stop`, and at a turn end the
   `finished` inbox item for that work. Its background threads go on in a
   fresh record (`Record::split_background`: a new key, `working.start`),
   so a later end of that record is reported under its own key and the
   inbox, which keeps one `finished` item per key, loses neither. A record
   with nothing but background work is left as it is; a record with no
   background threads ends as before. A publish addresses only the other
   threads unasked (`Working::foreground_threads_of`); the publish that
   carries a background thread's change names it in `addresses`.
3. **A background thread ends** when the owner replies to or resolves it,
   clears it (`done: true`, with or without `thread_ids`), when a viewer
   resolves or deletes it, when its session ends, or when the record
   lapses. A record that loses its last background thread and goes on is
   renewed then, so its 120 s lapse counts from that moment.
4. **A record naming background threads lapses after 600 s**
   (`BACKGROUND_TTL_SECS`) without renewal, not 120 s. In Claude Code and
   Codex a subagent's tool calls renew it (their `PostToolUse` carries the
   parent's `session_id`); in Grok Build and Pi's subagent packages,
   subagents are sessions of their own and nothing renews it until the
   owner acts again, so the longer lapse is what keeps it.
5. **Every harness the same way.** No hook reads the harness's
   background-task list; the change is the daemon, the `working` tool
   (Rust MCP server and Pi extension) and the skills.
6. **Codex: the owner waits.** Codex does not wake the parent when a child
   finishes (only its under-development `wake_on_agent_messages`
   feature does), so the skill tells a Codex owner to wait for its
   subagents (`wait_agent`, present in multi-agent v1 and, by default, v2)
   before ending a turn that has a thread in progress.

## Not done

- Ending at the turn end only when the harness lists no running subagent
  (Claude Code's `background_tasks`, Grok Build's `backgroundTasks`):
  session-wide, so it keeps threads the agent already answered, and does
  nothing for Codex or Pi.
- Per-subagent working records, keyed by harness agent IDs: needs a
  different record key and agent cooperation to name each subagent's
  threads; one owner record flagging threads is what the person needs.
- `SubagentStart`/`SubagentStop` counting for Claude Code: they fire again
  on every pause and resume, and one interactive run had a `SubagentStop`
  with no matching start.
- Codex waking the owner: revisit when `wake_on_agent_messages` is stable;
  the skill's wait rule can then go.
- Showing background threads differently on the page ("in the
  background, free for comments"): the view carries
  `background_thread_ids` for it; the page does not use it yet.

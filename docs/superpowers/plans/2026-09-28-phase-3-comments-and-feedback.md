# Artifax Phase 3: Comments and the Feedback Loop — Scoped Plan

> **Status:** scoped plan. Expand to step-level tasks with the writing-plans skill when phase 2 has shipped. Interfaces and acceptance criteria are the commitments.
>
> **For agentic workers:** when expanded, use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** A person comments on an element or text selection in a published page, sends the thread to the agent, and the publishing agent receives selector, quote, comment, and a PNG clip, then replies and resolves from its session.

**Architecture:** Threads with anchors and clips in SQLite and on disk; comment mode implemented in the bridge (hit testing, highlighting, clips) and the shell (composer, sidebar); feedback rows per target session with delivery tiers and acknowledgement; hooks for Stop and prompt-submit; `wait_for_feedback` long-poll; `codex queue` dispatch for Codex.

**Tech Stack:** phase 1–2 stack plus `modern-screenshot` in the bridge; `codex` CLI invoked by the daemon.

**Spec:** §5 (threads, comments, feedback, viewers), §6 (comments, feedback routes), §8 (comment affordances), §9 "Anchors", "Clips", §10 entire, §12 comments tools, §13 hooks additions, §15, §17 "Phase 3".

**Depends on:** phase 2 sessions and shim.

## Global constraints (additions)

- Only `sent_to_agent` threads accept agent replies and agent resolves; a plain thread returns guidance text, not an error.
- A feedback row is delivered once; `acknowledged_at` gates resends; at most three resends, marked `(resent)`.
- The feedback payload text format is exactly spec §10 "Feedback payload". Clip path is absolute and readable by the agent's file tools.
- `wait_for_feedback` default `timeout_s` 50, max 600; returns `{"feedback": [], "waited_s": n, "call_again": true}` on timeout.
- Comment bodies are untrusted; every tool result and hook payload wraps them in the labelled block.

## Pre-flight measurements (record results in `docs/contract.md`)

1. Codex: does `codex queue --thread <id> --message <text>` submit to an idle session, at end of turn, or only on user input? Measure with a live Codex session and a stopwatch; record the latency class.
2. Codex: does the `stop` hook honour `{"decision":"block","reason":...}` or an equivalent? Record the accepted output shape or "no".
3. Pi: does message injection exist (checklist item 4)? Record the API and whether it starts a turn.
4. Claude Code: confirm `Stop` hook input includes `stop_hook_active` and that `UserPromptSubmit` accepts `hookSpecificOutput.additionalContext`.

## Tasks

### Task 1: Storage for threads, comments, feedback, viewers
- Migration 3 per spec §5. `Store::create_thread(aid, version_n, anchor, author, body, clip: Option<bytes>) -> Thread`, `add_comment(thread, author_kind, author_name, via_session, body)`, `send_to_agent(thread) -> Vec<Feedback>` (targets: owner session + watching sessions that are live; else one untargeted row), `resolve_thread(thread, by)`, `list_threads(aid, include_resolved)`, `feedback_undelivered(session) -> Vec<Feedback>`, `mark_delivered(ids, tier)`, `feedback_for_resend(session, older_than 2 min, attempts < 3)`, `acknowledge(thread, session)`, `retarget_untargeted(aid, session)`, `upsert_viewer(cookie_id, name)`.
- Clips at `artifacts/<aid>/clips/<tid>.png`.
- Acceptance: unit tests for targeting rules (owner only, owner + watchers, no live session → untargeted then retargeted on next publish/watch), resend rules, and acknowledgement.

### Task 2: Watches
- Migration adds `watches`. `Store::watch(session, aid, replies_armed)`, `unwatch`, `list_watches(session)`, `watchers(aid)`; publish auto-watches the publishing session with `replies_armed = true`; session end removes its watches.
- Routes `PUT/DELETE /api/sessions/{sid}/watches/{aid}` (W), `GET`.
- Acceptance: integration tests; publish → watch row exists; `session-end` hook → watch gone.

### Task 3: Comment routes and SSE events
- Routes per spec §6 "Comments" and "Feedback": create thread (multipart: anchor JSON, body, optional clip PNG), add comment (viewer without token; agent with token and `author_kind=agent` only when `sent_to_agent`), send, resolve, list; `GET /api/sessions/{sid}/feedback?wait=<s>` long-poll using a `Notify` per session.
- New events: `thread`, `comment`, `thread_resolved`, `feedback_state` (for the "waiting for the agent" indicator: `{thread_id, state: sent|delivered|acknowledged|agent_ended, tier, since}`).
- Viewer identity: `artifax_viewer` cookie (ULID) set by the shell on first load; display name via `PUT /api/viewers/me`.
- Acceptance: integration tests including long-poll wake within 100 ms of a send; agent reply on a plain thread returns 200 with `{"guidance": ...}` and writes nothing.

### Task 4: Bridge comment mode, anchors, clips
- Bridge ↔ shell postMessage protocol: `artifax:hello` (bridge announces, shell replies with mode), `artifax:comment-mode {on}`, `artifax:hover {selector, rect}`, `artifax:pick {anchor, clipPng?}`, `artifax:resolve-anchors {anchors[]} → {results[]}`, `artifax:scroll-to {anchor}`. Bridge accepts messages only from `window.parent` with the shell's origin (passed in `data-shell-origin`, added to the bridge tag by the wrapper).
- Anchor builder: CSS path with `nth-of-type`, text quote with 32-char prefix/suffix, `sha256` of `outerHTML` (Web Crypto), rect with scroll offsets. Resolver in the spec's order.
- Clip: `modern-screenshot` `domToPng` of the anchored element (range → nearest block ancestor), DPR-aware, long side ≤ 1600 px; failures reported as `clipError` and the pick still completes.
- Acceptance: vitest with jsdom for anchor build/resolve (including a changed page where only the quote matches, and a detached case); Playwright for hover outline, element pick, range pick, and clip byte length > 0.

### Task 5: Shell comment UI
- Comment mode toggle, floating pin cursor, composer with quote and clip preview, thread sidebar (open, resolved, detached), pins overlaid on the frame at re-resolved rects, "Send to agent" button, resolve, viewer name field, "waiting for the agent" indicator with tier and elapsed time, `@agent` in a comment triggers send.
- Acceptance: Playwright: create element thread, see pin, send to agent, see "sent, waiting"; simulate agent reply via API and see it in the thread as `Agent · via claude`; resolve; republish and see the thread re-anchor or move to Detached.

### Task 6: MCP tools and tier 1 piggyback
- Tools `comments_read`, `comments_reply`, `comments_resolve`, `watch`, `wait_for_feedback` in the shim and HTTP MCP. `comments_read` returns threads with anchor summary, clip path, comments, `sent_to_agent`, status. Reading, replying, or resolving acknowledges feedback for that thread. Every tool result appends undelivered + resend-eligible feedback (tier 1) and marks it delivered/acknowledged.
- Acceptance: shim tests: send → next `list` call carries the block; `wait_for_feedback` returns within 1 s of a send and `call_again` on timeout; reply on a plain thread returns guidance.

### Task 7: Hooks: Stop and prompt-submit; `hook prompt` and `hook stop`
- Claude Code `Stop`: read `session_id`, `stop_hook_active`; fetch undelivered feedback for watched artifacts with `replies_armed`; if any, print `{"decision":"block","reason":"<payloads>"}` and mark delivered (`stop_hook`); else print nothing. `UserPromptSubmit`: print `hookSpecificOutput.additionalContext` with pending payloads (`prompt_hook`).
- Codex `stop`: same logic; output shape per pre-flight 2 or no-op.
- Acceptance: golden tests with fixtures including `stop_hook_active: true` with nothing new (allow) and with a new row (block once, then allow).

### Task 8: Tier 5 dispatch: Codex queue and Pi inject
- Daemon dispatcher: on `send`, for each target session with `replies_armed`: Codex with known `harness_session_id` → spawn `codex queue --thread <id> --message <payload>` (10 s timeout, async, exit code recorded, tier `queue`); Pi → if the extension registered an inject endpoint (it long-polls `GET /api/sessions/{sid}/inject`), deliver there (tier `inject`). Unknown thread ID → skip and mark session `push_disabled_reason`. Non-zero exit → end session, retarget rows, emit `feedback_state agent_ended`.
- `status` tool and `doctor --agent codex` report push state.
- Acceptance: unit tests with a fake `codex` binary on PATH (exit 0, exit 1, missing) asserting row states and session end; the live measurement from pre-flight 1 recorded in `docs/contract.md` with the observed latency class.

### Task 9: Plugin updates and skill
- Claude Code `hooks.json` adds `Stop` and `UserPromptSubmit`; commands `/artifax:comments`, `/artifax:watch`, `/artifax:wait`; skill section on the comment loop and the `wait_for_feedback` convention. Codex `hooks.snippet.toml` adds `stop`. Pi extension: `tool_result` appends pending feedback (tier 1 equivalent) and implements inject if available.

## Ship criteria
In Claude Code: publish → comment in the browser → send to agent → the agent's next tool result or stop hook carries the payload → `comments_reply` and `comments_resolve` appear in the browser. Codex behaviour measured and written into `docs/contract.md`. The shell's waiting indicator is truthful for every tier.
